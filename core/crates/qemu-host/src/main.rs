//! Host process for lcgamboa's QEMU fork built as a library (phase 0 spike).
//!
//! The fork exposes GPIO only through C callbacks registered in-process, with one
//! machine per process. This program loads the library, registers the callbacks,
//! and bridges them to a Unix socket so the core can stay in its own process.
//!
//! Usage: every command-line argument is passed to `qemu_init` unchanged, so the
//! core's session code can start this program in place of `qemu-system-*`.
//! Settings come from the environment:
//!
//! - `ESP32BENCH_QEMU_LIB`: path to `libqemu-xtensa.so` or `libqemu-riscv32.so`.
//! - `ESP32BENCH_GPIO_PINS` (optional): comma-separated GPIO numbers to bridge, e.g. `2,4`.
//! - `ESP32BENCH_GPIO_SOCK` (optional): Unix socket path to listen on for one client.
//!   Without it nothing is bridged: a plain simulation with nothing attached, where
//!   the I2C buses answer no address.
//! - `ESP32BENCH_CIRCUIT` (optional): a circuit file (schema/circuit) whose virtual
//!   parts run here (docs/parts.md). Its pins must be among the bridged ones.
//! - `ESP32BENCH_I2C_DEVICES` (optional): comma-separated `<bus>:<hex address>` pairs,
//!   e.g. `0:3c`. Each is a 256-byte register device: the first byte written after a
//!   start sets the register pointer, later bytes write registers, and reads return
//!   registers from the pointer on. Register `n` starts as `n + 0x40`. The fork's I2C
//!   callback is synchronous, so devices are modelled here rather than in the core.
//!
//! Socket protocol, one line per message:
//!
//! - host to client: `out <gpio> <0|1>` when the firmware drives a pin,
//!   `dir <gpio> <value>` when a pin's direction changes, and
//!   `i2c <bus> <addr> <ack|nack>` when the firmware starts a transfer.
//!   With a circuit: `part <id> <state as JSON>`, `picture <id> <width> <height>
//!   <base64, one byte per pixel>` (at most 20 a second per part) and
//!   `parterror <id> <message>`.
//! - client to host: `set <gpio> <0|1>` to drive an input pin, and with a circuit
//!   `control <id> <name> <value as JSON>` to work a part.

use std::collections::HashMap;
use std::ffi::{CString, c_char, c_int, c_void};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::UnixListener;
use std::process::ExitCode;
use std::sync::mpsc::{self, Sender};
use std::sync::{Mutex, OnceLock};

/// Layout of `callbacks_t` in `hw/xtensa/esp32_picsimlab.c` at commit 885a6e7061.
#[repr(C)]
struct Callbacks {
    write_pin: extern "C" fn(c_int, c_int),
    dir_pin: extern "C" fn(c_int, c_int),
    i2c_event: extern "C" fn(u8, u8, u16) -> c_int,
    spi_event: extern "C" fn(u8, u16) -> u8,
    uart_tx_event: extern "C" fn(u8, u8),
    pinmap: *const i16,
    rmt_event: extern "C" fn(u8, u32, u32),
}

type InitFn = unsafe extern "C" fn(c_int, *mut *mut c_char);
type MainLoopFn = unsafe extern "C" fn() -> c_int;
type CleanupFn = unsafe extern "C" fn(c_int);
type RegisterFn = unsafe extern "C" fn(*const c_void);
type SetPinFn = unsafe extern "C" fn(c_int, c_int);
type LockFn = unsafe extern "C" fn(*const c_char, c_int);
type UnlockFn = unsafe extern "C" fn();
type InternalsFn = unsafe extern "C" fn(c_int) -> *mut u32;

/// Functions resolved from the QEMU library.
struct Qemu {
    init: InitFn,
    main_loop: MainLoopFn,
    cleanup: CleanupFn,
    register_callbacks: RegisterFn,
    set_pin: SetPinFn,
    bql_lock: LockFn,
    bql_unlock: UnlockFn,
    internals: InternalsFn,
}

/// Messages to the bridge's worker thread: lines from QEMU's callbacks, and effects
/// of I2C parts (which run on QEMU's thread and must not drive pins from there).
enum Msg {
    Qemu(String),
    Client(String),
    Effects(Vec<esp32bench_circuit::Effect>),
    /// A WS2812 frame an RMT channel sent: the decoded bytes.
    Rmt {
        channel: u8,
        bytes: Vec<u8>,
    },
    Connected,
}

/// Pin events from QEMU's threads to the bridge worker.
static EVENTS: OnceLock<Sender<Msg>> = OnceLock::new();
/// The circuit's parts, when there is a circuit.
static PARTS: OnceLock<Mutex<esp32bench_circuit::Runtime>> = OnceLock::new();
/// Host pin index (1-based, as in the pin map) to GPIO number.
static GPIOS: OnceLock<Vec<i16>> = OnceLock::new();

fn gpio_for(index: c_int) -> Option<i16> {
    let i = usize::try_from(index).ok()?.checked_sub(1)?;
    GPIOS.get()?.get(i).copied()
}

fn emit(line: String) {
    if let Some(tx) = EVENTS.get() {
        let _ = tx.send(Msg::Qemu(line));
    }
}

fn parts_lock() -> Option<std::sync::MutexGuard<'static, esp32bench_circuit::Runtime>> {
    PARTS
        .get()
        .map(|m| m.lock().unwrap_or_else(std::sync::PoisonError::into_inner))
}

extern "C" fn on_write_pin(pin: c_int, value: c_int) {
    if let Some(gpio) = gpio_for(pin) {
        emit(format!("out {gpio} {}", i32::from(value != 0)));
    }
}

extern "C" fn on_dir_pin(pin: c_int, value: c_int) {
    // pin -1 is a "sync" notification for all pins; it carries no GPIO.
    if let Some(gpio) = gpio_for(pin) {
        emit(format!("dir {gpio} {value}"));
    }
}

// Event codes from QEMU's `enum i2c_event` (include/hw/i2c/i2c.h) and the fork's
// hw/i2c/picsimlab_i2c.c, which adds a byte write (`data << 8 | 5`) and a read (`6`).
const I2C_START_RECV: u16 = 0;
const I2C_START_SEND: u16 = 1;
const I2C_START_SEND_ASYNC: u16 = 2;
const I2C_WRITE_BYTE: u16 = 5;
const I2C_READ_BYTE: u16 = 6;

struct I2cDevice {
    regs: [u8; 256],
    pointer: u8,
    /// The next written byte is a register pointer, not data.
    expect_pointer: bool,
}

impl I2cDevice {
    fn new() -> Self {
        let mut regs = [0u8; 256];
        for (i, r) in regs.iter_mut().enumerate() {
            *r = (i as u8).wrapping_add(0x40);
        }
        I2cDevice {
            regs,
            pointer: 0,
            expect_pointer: false,
        }
    }

    /// Handle one bus event; the return value goes back to QEMU (0 = ACK, or the byte read).
    fn event(&mut self, event: u16) -> c_int {
        match event & 0xff {
            I2C_START_SEND | I2C_START_SEND_ASYNC => {
                self.expect_pointer = true;
                0
            }
            I2C_WRITE_BYTE => {
                let data = (event >> 8) as u8;
                if self.expect_pointer {
                    self.pointer = data;
                    self.expect_pointer = false;
                } else {
                    self.regs[usize::from(self.pointer)] = data;
                    self.pointer = self.pointer.wrapping_add(1);
                }
                0
            }
            I2C_READ_BYTE => {
                let v = self.regs[usize::from(self.pointer)];
                self.pointer = self.pointer.wrapping_add(1);
                c_int::from(v)
            }
            _ => 0, // START_RECV, FINISH, NACK
        }
    }
}

/// Modelled I2C devices by (bus, 7-bit address).
static I2C: OnceLock<Mutex<HashMap<(u8, u8), I2cDevice>>> = OnceLock::new();

extern "C" fn on_i2c_event(id: u8, addr: u8, event: u16) -> c_int {
    // A circuit part at this address answers first.
    if let Some(mut parts) = parts_lock()
        && parts.has_i2c(id, addr)
    {
        use esp32bench_circuit::I2cEvent;
        let ev = match event & 0xff {
            I2C_START_RECV => I2cEvent::Start { read: true },
            I2C_START_SEND | I2C_START_SEND_ASYNC => I2cEvent::Start { read: false },
            I2C_WRITE_BYTE => I2cEvent::Write((event >> 8) as u8),
            I2C_READ_BYTE => I2cEvent::Read,
            _ => I2cEvent::Stop, // FINISH, NACK
        };
        if let Some((answer, effects)) = parts.i2c(id, addr, ev) {
            drop(parts);
            if matches!(ev, I2cEvent::Start { .. }) {
                emit(format!(
                    "i2c {id} {addr:#04x} {}",
                    if answer == 0 { "ack" } else { "nack" }
                ));
            }
            if !effects.is_empty()
                && let Some(tx) = EVENTS.get()
            {
                let _ = tx.send(Msg::Effects(effects));
            }
            return answer;
        }
    }
    let Some(devices) = I2C.get() else {
        return 1;
    };
    let Ok(mut devices) = devices.lock() else {
        return 1;
    };
    let start = matches!(
        event & 0xff,
        I2C_START_RECV | I2C_START_SEND | I2C_START_SEND_ASYNC
    );
    match devices.get_mut(&(id, addr)) {
        Some(dev) => {
            if start {
                emit(format!("i2c {id} {addr:#04x} ack"));
            }
            dev.event(event)
        }
        // NACK the address so a scan only finds modelled devices.
        None => {
            if start {
                emit(format!("i2c {id} {addr:#04x} nack"));
            }
            1
        }
    }
}

extern "C" fn on_spi_event(_id: u8, _event: u16) -> u8 {
    0
}

extern "C" fn on_uart_tx(_id: u8, _value: u8) {}

/// RMT symbols decoded as WS2812 bits, per channel, until the transmission ends.
#[derive(Default, Clone)]
struct Ws2812 {
    bytes: Vec<u8>,
    byte: u8,
    bits: u8,
}

/// Whether a part listens for RMT frames (a WS2812 strip); else symbols are ignored.
static WANT_RMT: OnceLock<bool> = OnceLock::new();
static RMT: Mutex<[Option<Ws2812>; 8]> = Mutex::new([const { None }; 8]);
/// The library's `qemu_picsimlab_get_internals`, for the GPIO matrix's output routing.
static INTERNALS: OnceLock<InternalsFn> = OnceLock::new();
/// The GPIO matrix's output signal of RMT channel 0 (ESP32: 87; ESP32-C3: 51).
static RMT_SIGNAL0: OnceLock<u32> = OnceLock::new();
const QEMU_INTERNAL_GPIO_OUT_SEL: c_int = 2;
const OUT_SEL_GPIOS: usize = 40;

/// One RMT symbol from the fork's RMT models (patches 0007 and 0011), or 0 when a
/// transmission ended. A WS2812 bit is a high pulse then a low one: longer high for 1.
extern "C" fn on_rmt_event(channel: u8, _config0: u32, value: u32) {
    if !WANT_RMT.get().copied().unwrap_or(false) {
        return;
    }
    let i = usize::from(channel);
    if i >= 8 {
        return;
    }
    let mut rmt = RMT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let frame = rmt[i].get_or_insert_with(Ws2812::default);
    if value == 0 {
        let bytes = std::mem::take(&mut frame.bytes);
        *frame = Ws2812::default();
        if !bytes.is_empty()
            && let Some(tx) = EVENTS.get()
        {
            let _ = tx.send(Msg::Rmt { channel, bytes });
        }
        return;
    }
    let (high_first, d0, d1) = (value & 0x8000 != 0, value & 0x7fff, (value >> 16) & 0x7fff);
    if !high_first || frame.bytes.len() >= 3 * 1024 {
        return;
    }
    frame.byte = (frame.byte << 1) | u8::from(d0 > d1);
    frame.bits += 1;
    if frame.bits == 8 {
        frame.bytes.push(frame.byte);
        frame.byte = 0;
        frame.bits = 0;
    }
}

/// The GPIO an RMT channel is routed to, from the GPIO matrix's output selection.
fn rmt_gpio(channel: u8) -> Option<u8> {
    let internals = *INTERNALS.get()?;
    let signal = RMT_SIGNAL0.get()? + u32::from(channel);
    // SAFETY: the library returns its GPIO device's out_sel array (40 u32 on both
    // chips), which lives as long as the machine; it is read only.
    let table = unsafe { internals(QEMU_INTERNAL_GPIO_OUT_SEL) };
    if table.is_null() {
        return None;
    }
    (0..OUT_SEL_GPIOS)
        .find(|&g| {
            // SAFETY: g < 40, inside the array; a racy read of a register value is fine.
            let sel = unsafe { std::ptr::read_volatile(table.add(g)) };
            sel & 0x1ff == signal
        })
        .and_then(|g| u8::try_from(g).ok())
}

fn main() -> ExitCode {
    match run() {
        Ok(status) => ExitCode::from(u8::try_from(status).unwrap_or(1)),
        Err(e) => {
            eprintln!("esp32bench-qemu-host: {e}");
            ExitCode::FAILURE
        }
    }
}

fn env(name: &str) -> Result<std::ffi::OsString, String> {
    std::env::var_os(name).ok_or_else(|| format!("{name} is not set"))
}

fn run() -> Result<c_int, String> {
    let lib = env("ESP32BENCH_QEMU_LIB")?;
    let sock = std::env::var_os("ESP32BENCH_GPIO_SOCK");
    let gpios = match std::env::var_os("ESP32BENCH_GPIO_PINS") {
        Some(p) => parse_pins(&p.to_string_lossy())?,
        None => Vec::new(),
    };

    let i2c = match std::env::var_os("ESP32BENCH_I2C_DEVICES") {
        Some(v) => parse_i2c(&v.to_string_lossy())?,
        None => Vec::new(),
    };
    let _ = I2C.set(Mutex::new(
        i2c.into_iter().map(|k| (k, I2cDevice::new())).collect(),
    ));

    if let Some(path) = std::env::var_os("ESP32BENCH_CIRCUIT") {
        let data = std::fs::read(&path).map_err(|e| format!("reading {path:?}: {e}"))?;
        let circuit: esp32bench_circuit::Circuit =
            serde_json::from_slice(&data).map_err(|e| format!("{path:?}: {e}"))?;
        let runtime = esp32bench_circuit::Runtime::new(&circuit).map_err(|e| e.to_string())?;
        let _ = WANT_RMT.set(runtime.has_rmt());
        let _ = PARTS.set(Mutex::new(runtime));
    }

    let qemu = load(&lib)?;
    let _ = INTERNALS.set(qemu.internals);
    let _ = RMT_SIGNAL0.set(if lib.to_string_lossy().contains("riscv32") {
        51
    } else {
        87
    });

    // The pin map is read during machine set-up, so it must outlive qemu_init.
    let mut pinmap: Vec<i16> = Vec::with_capacity(gpios.len() + 1);
    pinmap.push(i16::try_from(gpios.len()).map_err(|_| "too many pins".to_string())?);
    pinmap.extend(&gpios);
    let pinmap: &'static [i16] = Box::leak(pinmap.into_boxed_slice());
    let _ = GPIOS.set(gpios);

    let callbacks: &'static Callbacks = Box::leak(Box::new(Callbacks {
        write_pin: on_write_pin,
        dir_pin: on_dir_pin,
        i2c_event: on_i2c_event,
        spi_event: on_spi_event,
        uart_tx_event: on_uart_tx,
        pinmap: pinmap.as_ptr(),
        rmt_event: on_rmt_event,
    }));

    if let Some(sock) = &sock {
        let listener = UnixListener::bind(sock).map_err(|e| format!("binding {sock:?}: {e}"))?;
        let (tx, rx) = mpsc::channel::<Msg>();
        let _ = EVENTS.set(tx.clone());
        start_bridge(
            listener,
            tx,
            rx,
            qemu.set_pin,
            qemu.bql_lock,
            qemu.bql_unlock,
        );
    }

    // argv for qemu_init: our own argv, unchanged. The strings must stay valid.
    let args: Vec<CString> = std::env::args_os()
        .map(|a| CString::new(a.as_bytes()).map_err(|_| "argument contains NUL".to_string()))
        .collect::<Result<_, _>>()?;
    let mut argv: Vec<*mut c_char> = args.iter().map(|a| a.as_ptr().cast_mut()).collect();
    argv.push(std::ptr::null_mut());
    let argc = c_int::try_from(args.len()).map_err(|_| "too many arguments".to_string())?;
    std::mem::forget(args);

    // SAFETY: the library is loaded, the callbacks and pin map are leaked and so live
    // for the rest of the process, and argv is a NULL-terminated array of valid,
    // leaked C strings, as qemu_init expects.
    let status = unsafe {
        (qemu.register_callbacks)(std::ptr::from_ref(callbacks).cast());
        (qemu.init)(argc, argv.as_mut_ptr());
        let status = (qemu.main_loop)();
        (qemu.cleanup)(status);
        status
    };
    if let Some(sock) = &sock {
        let _ = std::fs::remove_file(sock);
    }
    Ok(status)
}

fn parse_pins(s: &str) -> Result<Vec<i16>, String> {
    s.split(',')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(|p| {
            p.parse::<i16>()
                .ok()
                .filter(|g| (0..=48).contains(g))
                .ok_or_else(|| format!("bad GPIO number `{p}`"))
        })
        .collect()
}

fn parse_i2c(s: &str) -> Result<Vec<(u8, u8)>, String> {
    s.split(',')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (bus, addr) = p.split_once(':').ok_or(format!("bad I2C device `{p}`"))?;
            let bus = bus
                .parse::<u8>()
                .map_err(|_| format!("bad I2C bus in `{p}`"))?;
            let addr = u8::from_str_radix(addr.trim_start_matches("0x"), 16)
                .ok()
                .filter(|a| *a < 0x80)
                .ok_or(format!("bad I2C address in `{p}`"))?;
            Ok((bus, addr))
        })
        .collect()
}

/// Accept one client; send it pin events and apply its `set` commands. With a
/// circuit, also run the parts' pin and control hooks and apply their effects.
fn start_bridge(
    listener: UnixListener,
    tx: Sender<Msg>,
    rx: mpsc::Receiver<Msg>,
    set_pin: SetPinFn,
    bql_lock: LockFn,
    bql_unlock: UnlockFn,
) {
    std::thread::spawn(move || {
        let Ok((stream, _)) = listener.accept() else {
            return;
        };
        let Ok(reader) = stream.try_clone() else {
            return;
        };
        // Client lines go to the worker, in order with QEMU's.
        let client_tx = tx.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(reader).lines() {
                let Ok(line) = line else { break };
                if client_tx.send(Msg::Client(line)).is_err() {
                    break;
                }
            }
        });
        let _ = tx.send(Msg::Connected);
        let mut worker = Worker {
            out: stream,
            set_pin,
            bql_lock,
            bql_unlock,
            pictures: HashMap::new(),
            last_tick: None,
        };
        loop {
            match rx.recv_timeout(std::time::Duration::from_millis(PICTURE_MS)) {
                Ok(msg) => {
                    if !worker.handle(msg) {
                        break;
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
            worker.flush_pictures(false);
            worker.tick();
        }
    });
}

/// Pictures go out at most this often per part.
const PICTURE_MS: u64 = 50;

/// The latest picture of a part, and when one last went out.
struct PendingPicture {
    line: Option<String>,
    sent: std::time::Instant,
}

struct Worker {
    out: std::os::unix::net::UnixStream,
    set_pin: SetPinFn,
    bql_lock: LockFn,
    bql_unlock: UnlockFn,
    pictures: HashMap<String, PendingPicture>,
    last_tick: Option<std::time::Instant>,
}

impl Worker {
    fn send(&mut self, line: &str) -> bool {
        writeln!(self.out, "{line}").is_ok()
    }

    /// Drive a bridged GPIO as an input. This is not a QEMU thread, so take the lock:
    /// qemu_picsimlab_set_pin does not.
    fn drive(&self, gpio: i16, level: bool) {
        let Some(index) = GPIOS.get().and_then(|g| g.iter().position(|x| *x == gpio)) else {
            eprintln!("esp32bench-qemu-host: GPIO {gpio} is not bridged");
            return;
        };
        // Pin map indices are 1-based.
        let Ok(index) = c_int::try_from(index + 1) else {
            return;
        };
        // SAFETY: these are the library's own functions, called with the lock held
        // around the IRQ change as QEMU requires for non-QEMU threads.
        unsafe {
            (self.bql_lock)(c"esp32bench-qemu-host".as_ptr(), 0);
            (self.set_pin)(index, c_int::from(level));
            (self.bql_unlock)();
        }
    }

    fn apply(&mut self, effects: Vec<esp32bench_circuit::Effect>) -> bool {
        use base64::Engine;
        use esp32bench_circuit::Effect;
        for e in effects {
            let ok = match e {
                Effect::Drive { gpio, level } => {
                    self.drive(i16::from(gpio), level);
                    true
                }
                Effect::State { id, state } => self.send(&format!("part {id} {state}")),
                Effect::Picture {
                    id,
                    width,
                    height,
                    pixels,
                } => {
                    let line = format!(
                        "picture {id} {width} {height} {}",
                        base64::engine::general_purpose::STANDARD.encode(&pixels)
                    );
                    let p = self.pictures.entry(id).or_insert(PendingPicture {
                        line: None,
                        sent: std::time::Instant::now() - std::time::Duration::from_secs(1),
                    });
                    p.line = Some(line);
                    true
                }
                Effect::Error { id, message } => {
                    let message = message.replace('\n', " ");
                    self.send(&format!("parterror {id} {message}"))
                }
            };
            if !ok {
                return false;
            }
        }
        self.flush_pictures(false);
        true
    }

    /// Send pictures whose part has not sent one for PICTURE_MS (all, if `all`).
    fn flush_pictures(&mut self, all: bool) {
        let now = std::time::Instant::now();
        let mut due = Vec::new();
        for p in self.pictures.values_mut() {
            if p.line.is_some()
                && (all || now.duration_since(p.sent).as_millis() >= u128::from(PICTURE_MS))
            {
                p.sent = now;
                due.extend(p.line.take());
            }
        }
        for line in due {
            self.send(&line);
        }
    }

    /// Every 100 ms, the parts' `tick` hooks (clocks, alarms).
    fn tick(&mut self) {
        let now = std::time::Instant::now();
        if self
            .last_tick
            .is_some_and(|t| now.duration_since(t).as_millis() < 100)
        {
            return;
        }
        self.last_tick = Some(now);
        let effects = parts_lock().filter(|p| p.has_tick()).map(|mut p| p.tick());
        if let Some(e) = effects {
            self.apply(e);
        }
    }

    /// Returns false when the client has gone.
    fn handle(&mut self, msg: Msg) -> bool {
        match msg {
            Msg::Connected => {
                // The machine exists by now (the core connects once QEMU is up): set
                // the parts' starting levels and report their states.
                let effects = parts_lock().map(|mut p| {
                    let mut e = p.states();
                    e.extend(p.init());
                    e
                });
                effects.is_none_or(|e| self.apply(e))
            }
            Msg::Rmt { channel, bytes } => {
                let effects =
                    rmt_gpio(channel).and_then(|gpio| parts_lock().map(|mut p| p.rmt(gpio, bytes)));
                effects.is_none_or(|e| self.apply(e))
            }
            Msg::Qemu(line) => {
                if !self.send(&line) {
                    return false;
                }
                let mut it = line.split_whitespace();
                let (kind, gpio, value) = (it.next(), it.next(), it.next());
                let (Some(gpio), Some(value)) = (gpio.and_then(|g| g.parse::<u8>().ok()), value)
                else {
                    return true;
                };
                let v = value == "1";
                let effects = match kind {
                    Some("out") => parts_lock().map(|mut p| p.pin(gpio, Some(v), None)),
                    Some("dir") => parts_lock().map(|mut p| p.pin(gpio, None, Some(v))),
                    _ => None,
                };
                effects.is_none_or(|e| self.apply(e))
            }
            Msg::Effects(e) => self.apply(e),
            Msg::Client(line) => {
                let mut it = line.splitn(4, ' ');
                match (it.next(), it.next(), it.next(), it.next()) {
                    (Some("set"), Some(gpio), Some(level), None) => {
                        match gpio.parse::<i16>() {
                            Ok(g) => self.drive(g, level != "0"),
                            Err(_) => eprintln!("esp32bench-qemu-host: ignoring `{line}`"),
                        }
                        true
                    }
                    (Some("control"), Some(id), Some(name), Some(value)) => {
                        let value: rhai::Dynamic =
                            match serde_json::from_str::<serde_json::Value>(value) {
                                Ok(serde_json::Value::Bool(b)) => b.into(),
                                Ok(serde_json::Value::Number(n)) => {
                                    n.as_f64().unwrap_or(0.0).into()
                                }
                                _ => {
                                    return self.send(&format!("parterror {id} bad control value"));
                                }
                            };
                        let r = parts_lock().map(|mut p| p.control(id, name, value));
                        match r {
                            Some(Ok(e)) => self.apply(e),
                            Some(Err(e)) => self.send(&format!("parterror {id} {e}")),
                            None => self.send(&format!("parterror {id} there is no circuit")),
                        }
                    }
                    _ => {
                        eprintln!("esp32bench-qemu-host: ignoring `{line}`");
                        true
                    }
                }
            }
        }
    }
}

/// dlopen the library and resolve the functions we use.
fn load(path: &std::ffi::OsStr) -> Result<Qemu, String> {
    let cpath =
        CString::new(path.as_bytes()).map_err(|_| "library path contains NUL".to_string())?;
    // SAFETY: dlopen with a valid C string. The handle is never closed, so resolved
    // symbols stay valid for the life of the process.
    let handle = unsafe { libc::dlopen(cpath.as_ptr(), libc::RTLD_NOW | libc::RTLD_GLOBAL) };
    if handle.is_null() {
        return Err(format!("dlopen {path:?}: {}", dl_error()));
    }
    let sym = |name: &std::ffi::CStr| -> Result<*mut c_void, String> {
        // SAFETY: handle is a live dlopen handle and name is a valid C string.
        let p = unsafe { libc::dlsym(handle, name.as_ptr()) };
        if p.is_null() {
            Err(format!("{path:?} has no symbol {name:?}"))
        } else {
            Ok(p)
        }
    };
    // SAFETY: each symbol is a function with the C signature written here, taken from
    // the fork's sources (system/main.c, hw/xtensa/esp32_picsimlab.c, system/cpus.c).
    unsafe {
        Ok(Qemu {
            init: std::mem::transmute::<*mut c_void, InitFn>(sym(c"qemu_init")?),
            main_loop: std::mem::transmute::<*mut c_void, MainLoopFn>(sym(c"qemu_main_loop")?),
            cleanup: std::mem::transmute::<*mut c_void, CleanupFn>(sym(c"qemu_cleanup")?),
            register_callbacks: std::mem::transmute::<*mut c_void, RegisterFn>(sym(
                c"qemu_picsimlab_register_callbacks",
            )?),
            set_pin: std::mem::transmute::<*mut c_void, SetPinFn>(sym(c"qemu_picsimlab_set_pin")?),
            bql_lock: std::mem::transmute::<*mut c_void, LockFn>(sym(c"bql_lock_impl")?),
            bql_unlock: std::mem::transmute::<*mut c_void, UnlockFn>(sym(c"bql_unlock")?),
            internals: std::mem::transmute::<*mut c_void, InternalsFn>(sym(
                c"qemu_picsimlab_get_internals",
            )?),
        })
    }
}

fn dl_error() -> String {
    // SAFETY: dlerror returns NULL or a valid C string owned by libc.
    let e = unsafe { libc::dlerror() };
    if e.is_null() {
        "unknown error".into()
    } else {
        // SAFETY: non-null pointer from dlerror, valid until the next dl* call.
        unsafe { std::ffi::CStr::from_ptr(e) }
            .to_string_lossy()
            .into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pin_lists() {
        assert_eq!(parse_pins("2,4").unwrap(), vec![2, 4]);
        assert_eq!(parse_pins(" 2 , 4 ,").unwrap(), vec![2, 4]);
        assert!(parse_pins("2,x").is_err());
        assert!(parse_pins("99").is_err());
    }

    #[test]
    fn parses_i2c_devices() {
        assert_eq!(
            parse_i2c("0:3c, 1:0x50").unwrap(),
            vec![(0, 0x3c), (1, 0x50)]
        );
        assert!(parse_i2c("0:80").is_err());
        assert!(parse_i2c("3c").is_err());
    }

    #[test]
    fn i2c_device_models_a_register_file() {
        let mut d = I2cDevice::new();
        // Write pointer 0x10, then read two bytes.
        assert_eq!(d.event(I2C_START_SEND), 0);
        assert_eq!(d.event((0x10 << 8) | I2C_WRITE_BYTE), 0);
        assert_eq!(d.event(I2C_START_RECV), 0);
        assert_eq!(d.event(I2C_READ_BYTE), 0x50);
        assert_eq!(d.event(I2C_READ_BYTE), 0x51);
        // Write pointer 0x20 and data 0xAB, then read it back.
        d.event(I2C_START_SEND);
        d.event((0x20 << 8) | I2C_WRITE_BYTE);
        d.event((0xAB << 8) | I2C_WRITE_BYTE);
        d.event(I2C_START_SEND);
        d.event((0x20 << 8) | I2C_WRITE_BYTE);
        d.event(I2C_START_RECV);
        assert_eq!(d.event(I2C_READ_BYTE), 0xAB);
    }

    #[test]
    fn callbacks_struct_matches_c_layout() {
        // Six function pointers and one data pointer, no padding.
        assert_eq!(
            std::mem::size_of::<Callbacks>(),
            7 * std::mem::size_of::<usize>()
        );
        assert_eq!(std::mem::offset_of!(Callbacks, pinmap), 5 * 8);
    }
}
