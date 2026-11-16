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
//! - `ESP32D_QEMU_LIB`: path to `libqemu-xtensa.so` or `libqemu-riscv32.so`.
//! - `ESP32D_GPIO_PINS`: comma-separated GPIO numbers to bridge, e.g. `2,4`.
//! - `ESP32D_GPIO_SOCK`: Unix socket path to listen on for one client.
//!
//! Socket protocol, one line per message:
//!
//! - host to client: `out <gpio> <0|1>` when the firmware drives a pin, and
//!   `dir <gpio> <value>` when a pin's direction changes.
//! - client to host: `set <gpio> <0|1>` to drive an input pin.

use std::ffi::{CString, c_char, c_int, c_void};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::UnixListener;
use std::process::ExitCode;
use std::sync::OnceLock;
use std::sync::mpsc::{self, Sender};

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

/// Functions resolved from the QEMU library.
struct Qemu {
    init: InitFn,
    main_loop: MainLoopFn,
    cleanup: CleanupFn,
    register_callbacks: RegisterFn,
    set_pin: SetPinFn,
    bql_lock: LockFn,
    bql_unlock: UnlockFn,
}

/// Pin events from QEMU's threads to the socket writer.
static EVENTS: OnceLock<Sender<String>> = OnceLock::new();
/// Host pin index (1-based, as in the pin map) to GPIO number.
static GPIOS: OnceLock<Vec<i16>> = OnceLock::new();

fn gpio_for(index: c_int) -> Option<i16> {
    let i = usize::try_from(index).ok()?.checked_sub(1)?;
    GPIOS.get()?.get(i).copied()
}

fn emit(line: String) {
    if let Some(tx) = EVENTS.get() {
        let _ = tx.send(line);
    }
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

extern "C" fn on_i2c_event(_id: u8, _addr: u8, _event: u16) -> c_int {
    0
}

extern "C" fn on_spi_event(_id: u8, _event: u16) -> u8 {
    0
}

extern "C" fn on_uart_tx(_id: u8, _value: u8) {}

extern "C" fn on_rmt_event(_channel: u8, _config0: u32, _value: u32) {}

fn main() -> ExitCode {
    match run() {
        Ok(status) => ExitCode::from(u8::try_from(status).unwrap_or(1)),
        Err(e) => {
            eprintln!("esp32d-qemu-host: {e}");
            ExitCode::FAILURE
        }
    }
}

fn env(name: &str) -> Result<std::ffi::OsString, String> {
    std::env::var_os(name).ok_or_else(|| format!("{name} is not set"))
}

fn run() -> Result<c_int, String> {
    let lib = env("ESP32D_QEMU_LIB")?;
    let sock = env("ESP32D_GPIO_SOCK")?;
    let gpios = parse_pins(&env("ESP32D_GPIO_PINS")?.to_string_lossy())?;

    let qemu = load(&lib)?;

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

    let listener = UnixListener::bind(&sock).map_err(|e| format!("binding {sock:?}: {e}"))?;
    let (tx, rx) = mpsc::channel::<String>();
    let _ = EVENTS.set(tx);
    start_bridge(listener, rx, qemu.set_pin, qemu.bql_lock, qemu.bql_unlock);

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
    let _ = std::fs::remove_file(&sock);
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

/// Accept one client; send it pin events and apply its `set` commands.
fn start_bridge(
    listener: UnixListener,
    rx: mpsc::Receiver<String>,
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
        std::thread::spawn(move || {
            let mut out = stream;
            for line in rx {
                if writeln!(out, "{line}").is_err() {
                    break;
                }
            }
        });
        for line in BufReader::new(reader).lines() {
            let Ok(line) = line else { break };
            let mut parts = line.split_whitespace();
            let (Some("set"), Some(gpio), Some(level)) = (parts.next(), parts.next(), parts.next())
            else {
                eprintln!("esp32d-qemu-host: ignoring `{line}`");
                continue;
            };
            let Some(index) = gpio
                .parse::<i16>()
                .ok()
                .and_then(|g| GPIOS.get()?.iter().position(|x| *x == g))
            else {
                eprintln!("esp32d-qemu-host: GPIO {gpio} is not bridged");
                continue;
            };
            let level = c_int::from(level != "0");
            // Pin map indices are 1-based. qemu_picsimlab_set_pin does not take the
            // big QEMU lock itself, so take it here; this thread is not a QEMU thread.
            let Ok(index) = c_int::try_from(index + 1) else {
                continue;
            };
            // SAFETY: these are the library's own functions, called with the lock held
            // around the IRQ change as QEMU requires for non-QEMU threads.
            unsafe {
                bql_lock(c"esp32d-qemu-host".as_ptr(), 0);
                set_pin(index, level);
                bql_unlock();
            }
        }
    });
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
    fn callbacks_struct_matches_c_layout() {
        // Six function pointers and one data pointer, no padding.
        assert_eq!(
            std::mem::size_of::<Callbacks>(),
            7 * std::mem::size_of::<usize>()
        );
        assert_eq!(std::mem::offset_of!(Callbacks, pinmap), 5 * 8);
    }
}
