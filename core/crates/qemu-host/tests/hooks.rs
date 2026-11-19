//! lcgamboa's QEMU library hooks through esp32bench-qemu-host:
//!
//! - GPIO: the host sees the firmware's output pin toggle, and the firmware sees an
//!   input pin the host drives (`tests/firmware/gpio`).
//! - I2C: the firmware's bus scan finds only the host's modelled device, and reads
//!   its registers (`tests/firmware/i2c-scan`).
//!
//! Needs a library build of the fork (`build_libqemu-esp32.sh`) and the test
//! firmware (`tests/firmware/build.sh gpio`, `... i2c-scan`). Set `ESP32BENCH_TEST_QEMU_LIB_XTENSA`
//! and/or `ESP32BENCH_TEST_QEMU_LIB_RISCV32` to `<fork>/build/libqemu-*.so`, or build
//! the fork with `esp32bench toolchain install qemu-lcgamboa`; the ROMs
//! are taken from `<fork>/pc-bios`. Without them the test is skipped.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use esp32bench_sim::uart::WaitOutcome;
use esp32bench_sim::{Chip, Session, SessionConfig};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::mpsc;

/// Start `project` through the host with the given extra environment. Returns the
/// session and the host's socket path, or None if the library or image is missing.
async fn start(
    chip: Chip,
    machine: &str,
    lib_var: &str,
    project: &str,
    extra_env: &[(&str, &str)],
) -> Option<(Session, PathBuf)> {
    // The variable wins; otherwise the build `esp32bench toolchain install
    // qemu-lcgamboa` made.
    let Some(lib) = std::env::var_os(lib_var)
        .map(PathBuf::from)
        .or_else(|| esp32bench_sim::lcgamboa::Build::find().map(|b| b.library(chip.arch())))
    else {
        eprintln!("skipping {project} on {chip}: {lib_var} not set and the fork is not built");
        return None;
    };
    let image = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../tests/firmware/out")
        .join(format!("{project}-{chip}.bin"));
    if !image.is_file() {
        eprintln!(
            "skipping {project} on {chip}: {} not built",
            image.display()
        );
        return None;
    }
    let pc_bios = lib
        .parent()
        .and_then(Path::parent)
        .map(|tree| tree.join("pc-bios"))
        .unwrap();
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("esp32bench");
    std::fs::create_dir_all(&runtime).unwrap();
    let sock = runtime.join(format!("{project}-test-{}-{chip}.sock", std::process::id()));
    let _ = std::fs::remove_file(&sock);

    let mut env: Vec<(OsString, OsString)> = vec![
        ("ESP32BENCH_QEMU_LIB".into(), lib.into()),
        ("ESP32BENCH_GPIO_PINS".into(), "2,4".into()),
        ("ESP32BENCH_GPIO_SOCK".into(), sock.clone().into()),
    ];
    env.extend(
        extra_env
            .iter()
            .map(|(k, v)| (OsString::from(k), OsString::from(v))),
    );
    let cfg = SessionConfig {
        chip,
        qemu: env!("CARGO_BIN_EXE_esp32bench-qemu-host").into(),
        image,
        gdb_port: None,
        start_paused: false,
        machine: Some(machine.into()),
        extra_args: vec!["-L".into(), pc_bios.into()],
        env,
        // These tests set the host's pins and socket themselves.
        bridge: Vec::new(),
        circuit: None,
        efuse: None,
        usb_console: false,
        network: None,
        icount: false,
    };
    Some((Session::start(cfg).await.unwrap(), sock))
}

async fn round_trip(chip: Chip, machine: &str, lib_var: &str) {
    let Some((mut s, sock)) = start(chip, machine, lib_var, "gpio", &[]).await else {
        return;
    };

    let gpio = UnixStream::connect(&sock).await.unwrap();
    let (r, mut w) = gpio.into_split();
    let (tx, mut events) = mpsc::unbounded_channel::<String>();
    tokio::spawn(async move {
        let mut lines = BufReader::new(r).lines();
        while let Ok(Some(l)) = lines.next_line().await {
            let _ = tx.send(l);
        }
    });

    let t = Duration::from_secs(30);
    let ready = s
        .uart
        .wait_for(b"esp32bench-gpio ready", t, |_| {})
        .await
        .unwrap();
    assert_eq!(ready, WaitOutcome::Found, "{chip}: firmware not ready");

    // The firmware toggles GPIO 2 every 500 ms; expect both levels.
    let mut seen = (false, false);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !(seen.0 && seen.1) {
        let ev = tokio::time::timeout_at(deadline, events.recv())
            .await
            .unwrap_or_else(|_| panic!("{chip}: GPIO 2 did not toggle; seen {seen:?}"))
            .unwrap();
        // `out <gpio> <level> <simulated ns>`
        let words: Vec<&str> = ev.split(' ').collect();
        match words.as_slice() {
            ["out", "2", "0", t] if t.parse::<u64>().is_ok() => seen.0 = true,
            ["out", "2", "1", t] if t.parse::<u64>().is_ok() => seen.1 = true,
            _ => {}
        }
    }

    for (cmd, marker) in [
        (&b"set 4 1\n"[..], &b"esp32bench-gpio in 4=1"[..]),
        (&b"set 4 0\n"[..], &b"esp32bench-gpio in 4=0"[..]),
    ] {
        w.write_all(cmd).await.unwrap();
        let got = s
            .uart
            .wait_for(marker, Duration::from_secs(5), |_| {})
            .await
            .unwrap();
        assert_eq!(
            got,
            WaitOutcome::Found,
            "{chip}: firmware did not see {cmd:?}"
        );
    }

    s.shutdown().await.unwrap();
    let _ = std::fs::remove_file(&sock);
}

#[tokio::test]
async fn esp32_gpio_round_trip() {
    round_trip(
        Chip::Esp32,
        "esp32-picsimlab",
        "ESP32BENCH_TEST_QEMU_LIB_XTENSA",
    )
    .await;
}

#[tokio::test]
async fn esp32c3_gpio_round_trip() {
    round_trip(
        Chip::Esp32c3,
        "esp32c3-picsimlab",
        "ESP32BENCH_TEST_QEMU_LIB_RISCV32",
    )
    .await;
}

/// The firmware scans I2C bus 0 and reads two registers from 0x3C. The host models
/// one device there; register n holds n + 0x40.
async fn i2c_device(chip: Chip, machine: &str, lib_var: &str) {
    let Some((mut s, sock)) = start(
        chip,
        machine,
        lib_var,
        "i2c-scan",
        &[("ESP32BENCH_I2C_DEVICES", "0:3c")],
    )
    .await
    else {
        return;
    };
    let mut uart = Vec::new();
    let got = s
        .uart
        .wait_for(b"esp32bench-i2c read", Duration::from_secs(30), |b| {
            uart.extend_from_slice(b)
        })
        .await
        .unwrap();
    // Let the rest of the line arrive.
    let _ = s
        .uart
        .wait_for(b"\n", Duration::from_secs(2), |b| uart.extend_from_slice(b))
        .await;
    let text = String::from_utf8_lossy(&uart);
    assert_eq!(got, WaitOutcome::Found, "{chip}: {text}");
    assert!(
        text.contains("esp32bench-i2c found 0x3c"),
        "{chip}: scan missed 0x3c: {text}"
    );
    assert!(
        text.contains("esp32bench-i2c scan done count=1"),
        "{chip}: scan found other devices: {text}"
    );
    assert!(
        text.contains("esp32bench-i2c read 0x3c reg 0x10 = 0x50 0x51"),
        "{chip}: wrong register values: {text}"
    );
    s.shutdown().await.unwrap();
    let _ = std::fs::remove_file(&sock);
}

#[tokio::test]
async fn esp32_i2c_device() {
    i2c_device(
        Chip::Esp32,
        "esp32-picsimlab",
        "ESP32BENCH_TEST_QEMU_LIB_XTENSA",
    )
    .await;
}

#[tokio::test]
async fn esp32c3_i2c_device() {
    i2c_device(
        Chip::Esp32c3,
        "esp32c3-picsimlab",
        "ESP32BENCH_TEST_QEMU_LIB_RISCV32",
    )
    .await;
}

/// The simulator's own launch (no pins, no socket, no devices): an I2C scan finds
/// nothing, as on a board with nothing attached. The fork's plain executables would
/// ACK every address, and Espressif's QEMU shows a TMP105 at 0x48 on the ESP32.
async fn empty_bus_scans_empty(chip: Chip) {
    let Some(build) = esp32bench_sim::lcgamboa::Build::find() else {
        eprintln!("skipping {chip}: lcgamboa's QEMU is not built");
        return;
    };
    let image = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(format!("../../../tests/firmware/out/i2c-scan-{chip}.bin"));
    if !image.is_file() {
        eprintln!("skipping {chip}: i2c-scan not built");
        return;
    }
    let host = PathBuf::from(env!("CARGO_BIN_EXE_esp32bench-qemu-host"));
    let launch = esp32bench_sim::engine::lcgamboa_launch(chip, &build, Some(&host)).unwrap();
    let mut s = Session::start(launch.config(chip, image)).await.unwrap();
    let mut text = Vec::new();
    let found = s
        .uart
        .wait_for(b"scan done count=", Duration::from_secs(30), |b| {
            text.extend_from_slice(b)
        })
        .await
        .unwrap();
    assert_eq!(found, WaitOutcome::Found);
    // The count follows the marker.
    let rest = s
        .uart
        .wait_for(b"\n", Duration::from_secs(5), |b| text.extend_from_slice(b))
        .await
        .unwrap();
    assert_eq!(rest, WaitOutcome::Found);
    let text = String::from_utf8_lossy(&text);
    assert!(text.contains("scan done count=0"), "{text}");
    s.shutdown().await.unwrap();
}

#[tokio::test]
async fn esp32_empty_bus_scans_empty() {
    empty_bus_scans_empty(Chip::Esp32).await;
}

#[tokio::test]
async fn esp32c3_empty_bus_scans_empty() {
    empty_bus_scans_empty(Chip::Esp32c3).await;
}

/// The simulator's own pin link (`SessionConfig::bridge` and `Session::gpio`): the
/// gpio firmware's GPIO 2 output is seen toggling, and driving GPIO 4 reaches it.
async fn session_bridges_pins(chip: Chip) {
    use esp32bench_sim::gpio::PinEvent;
    let Some(build) = esp32bench_sim::lcgamboa::Build::find() else {
        eprintln!("skipping {chip}: lcgamboa's QEMU is not built");
        return;
    };
    let image = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(format!("../../../tests/firmware/out/gpio-{chip}.bin"));
    if !image.is_file() {
        eprintln!("skipping {chip}: gpio firmware not built");
        return;
    }
    let host = PathBuf::from(env!("CARGO_BIN_EXE_esp32bench-qemu-host"));
    let launch = esp32bench_sim::engine::lcgamboa_launch(chip, &build, Some(&host)).unwrap();
    assert!(launch.hooks);
    let mut cfg = launch.config(chip, image);
    cfg.bridge = vec![2, 4];
    let mut s = Session::start(cfg).await.unwrap();
    let mut gpio = s.gpio.take().expect("no pin link");
    let mut events = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    let seen = |events: &[PinEvent], want: bool| {
        events
            .iter()
            .any(|e| matches!(e, PinEvent::Level { gpio: 2, level, .. } if *level == want))
    };
    while !(seen(&events, true) && seen(&events, false)) {
        let ev = tokio::time::timeout_at(deadline, gpio.next_event())
            .await
            .expect("GPIO 2 did not toggle")
            .unwrap()
            .expect("host closed the link");
        events.push(ev);
    }
    assert!(
        events.contains(&PinEvent::Direction {
            gpio: 2,
            output: true
        }),
        "{events:?}"
    );
    // Each edge has the simulation's time, which does not go backwards.
    let times: Vec<u64> = events
        .iter()
        .filter_map(|e| match e {
            PinEvent::Level { time_ns, .. } => Some(time_ns.expect("an edge without a time")),
            _ => None,
        })
        .collect();
    assert!(times.windows(2).all(|w| w[0] <= w[1]), "{times:?}");
    assert!(times.last().is_some_and(|t| *t > 0), "{times:?}");
    // Drive the input: the host reports it, timed, for the trace...
    gpio.set(4, true).await.unwrap();
    let input = loop {
        let ev = tokio::time::timeout(Duration::from_secs(10), gpio.next_event())
            .await
            .expect("the host did not report the driven input")
            .unwrap()
            .expect("host closed the link");
        if let PinEvent::Input {
            gpio: 4,
            level,
            time_ns,
        } = ev
        {
            break (level, time_ns);
        }
    };
    assert!(input.0);
    assert!(
        input.1.is_some_and(|t| t >= times[times.len() - 1]),
        "{input:?}"
    );
    // ...and the firmware sees it.
    let mut text = Vec::new();
    let found = s
        .uart
        .wait_for(b"esp32bench-gpio in 4=1", Duration::from_secs(10), |c| {
            text.extend_from_slice(c)
        })
        .await
        .unwrap();
    assert_eq!(
        found,
        WaitOutcome::Found,
        "{chip}: {}",
        String::from_utf8_lossy(&text)
    );
    gpio.set(4, false).await.unwrap();
    let found = s
        .uart
        .wait_for(b"esp32bench-gpio in 4=0", Duration::from_secs(10), |_| {})
        .await
        .unwrap();
    assert_eq!(found, WaitOutcome::Found);
    s.shutdown().await.unwrap();
}

#[tokio::test]
async fn esp32_session_bridges_pins() {
    session_bridges_pins(Chip::Esp32).await;
}

#[tokio::test]
async fn esp32c3_session_bridges_pins() {
    session_bridges_pins(Chip::Esp32c3).await;
}

/// The SSD1306 part model on a real I2C bus: tests/firmware/ssd1306 sends Adafruit's
/// init sequence and a frame in 32-byte chunks, and the host's part draws it.
async fn ssd1306_part_draws(chip: Chip) {
    use esp32bench_sim::gpio::PinEvent;
    let Some(build) = esp32bench_sim::lcgamboa::Build::find() else {
        eprintln!("skipping {chip}: lcgamboa's QEMU is not built");
        return;
    };
    let image = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(format!("../../../tests/firmware/out/ssd1306-{chip}.bin"));
    if !image.is_file() {
        eprintln!("skipping {chip}: ssd1306 firmware not built");
        return;
    }
    let host = PathBuf::from(env!("CARGO_BIN_EXE_esp32bench-qemu-host"));
    let launch = esp32bench_sim::engine::lcgamboa_launch(chip, &build, Some(&host)).unwrap();
    let mut cfg = launch.config(chip, image);
    cfg.circuit = Some(
        r#"{"schemaVersion":"1.1.0","parts":[{"id":"oled","kind":"ssd1306","i2c":{"bus":0,"address":60}}]}"#
            .into(),
    );
    let mut s = Session::start(cfg).await.unwrap();
    let mut link = s.gpio.take().expect("no link to the host");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    // Read UART0 all along: QEMU must not be left with output nobody takes.
    let mut text = Vec::new();
    let mut drawn = false;
    while !(drawn && String::from_utf8_lossy(&text).contains("esp32bench-ssd1306 ready")) {
        tokio::select! {
            chunk = s.uart.read_chunk() => {
                if let Ok(Some(c)) = chunk { text.extend_from_slice(&c) }
            }
            ev = link.next_event() => match ev.unwrap().expect("host closed the link") {
                PinEvent::PartError { id, message } => panic!("{chip}: {id}: {message}"),
                PinEvent::Picture { width, height, pixels, .. } => {
                    assert_eq!((width, height), (128, 64));
                    if pixels.iter().filter(|p| **p != 0).count() == 128 * 8 + 1 {
                        let at = |x: usize, y: usize| pixels[y * 128 + x];
                        assert!((0..128).all(|x| (0..8).all(|y| at(x, y) == 1)), "{chip}: top page");
                        assert_eq!(at(64, 32), 1, "{chip}: the lone pixel");
                        assert_eq!(at(64, 33), 0);
                        drawn = true;
                    }
                }
                _ => {}
            },
            () = tokio::time::sleep_until(deadline) => panic!(
                "{chip}: drawn={drawn}; uart:\n{}", String::from_utf8_lossy(&text)
            ),
        }
    }
    s.shutdown().await.unwrap();
}

#[tokio::test]
async fn esp32_ssd1306_part_draws() {
    ssd1306_part_draws(Chip::Esp32).await;
}

#[tokio::test]
async fn esp32c3_ssd1306_part_draws() {
    ssd1306_part_draws(Chip::Esp32c3).await;
}

/// The last `esp32bench-bme280 t=.. h=.. p=..` line in `text`, as (t, h, p).
fn last_reading(text: &str) -> Option<(i64, i64, i64)> {
    let line = text
        .lines()
        .rev()
        .find(|l| l.contains("esp32bench-bme280 t="))?;
    let num = |key: &str| -> Option<i64> {
        let rest = &line[line.find(key)? + key.len()..];
        rest.split_whitespace().next()?.parse().ok()
    };
    Some((num("t=")?, num("h=")?, num("p=")?))
}

/// Within one unit of the firmware's integer output (it truncates).
fn near(got: (i64, i64, i64), want: (i64, i64, i64)) -> bool {
    (got.0 - want.0).abs() <= 1 && (got.1 - want.1).abs() <= 1 && (got.2 - want.2).abs() <= 1
}

/// The BME280 part model on a real I2C bus: tests/firmware/bme280 reads it with the
/// datasheet's compensation, and a control moves the reading.
async fn bme280_part_reads(chip: Chip) {
    let Some(build) = esp32bench_sim::lcgamboa::Build::find() else {
        eprintln!("skipping {chip}: lcgamboa's QEMU is not built");
        return;
    };
    let image = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(format!("../../../tests/firmware/out/bme280-{chip}.bin"));
    if !image.is_file() {
        eprintln!("skipping {chip}: bme280 firmware not built");
        return;
    }
    let host = PathBuf::from(env!("CARGO_BIN_EXE_esp32bench-qemu-host"));
    let launch = esp32bench_sim::engine::lcgamboa_launch(chip, &build, Some(&host)).unwrap();
    let mut cfg = launch.config(chip, image);
    cfg.circuit = Some(
        r#"{"schemaVersion":"1.1.0","parts":[{"id":"env","kind":"bme280","i2c":{"bus":0,"address":118}}]}"#
            .into(),
    );
    let mut s = Session::start(cfg).await.unwrap();
    let mut link = s.gpio.take().expect("no link to the host");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let mut text = String::new();
    let mut moved = false;
    // UART0 and the host's link are both read all along.
    // 21 degC, 45 %RH, 1013.25 hPa, in the firmware's units (0.01 degC, 0.01 %, Pa).
    let defaults = (2100, 4500, 101325);
    while !(moved && last_reading(&text).is_some_and(|r| near(r, (3050, 4500, 101325)))) {
        tokio::select! {
            chunk = s.uart.read_chunk() => {
                if let Ok(Some(c)) = chunk { text.push_str(&String::from_utf8_lossy(&c)); }
                if !moved && last_reading(&text).is_some_and(|r| near(r, defaults)) {
                    assert!(text.contains("esp32bench-bme280 ready id=0x60"), "{chip}: {text}");
                    link.control("env", "temperature", &serde_json::json!(30.5)).await.unwrap();
                    moved = true;
                }
            }
            ev = link.next_event() => {
                if let esp32bench_sim::gpio::PinEvent::PartError { id, message } = ev.unwrap().expect("host closed the link") {
                    panic!("{chip}: {id}: {message}");
                }
            }
            () = tokio::time::sleep_until(deadline) => panic!("{chip}: moved={moved}; uart:\n{text}"),
        }
    }
    s.shutdown().await.unwrap();
}

#[tokio::test]
async fn esp32_bme280_part_reads() {
    bme280_part_reads(Chip::Esp32).await;
}

#[tokio::test]
async fn esp32c3_bme280_part_reads() {
    bme280_part_reads(Chip::Esp32c3).await;
}

/// The MPU-6050 part model on a real I2C bus: tests/firmware/mpu6050 wakes it and
/// reads the 14-byte burst; flat it reads +1 g on z, and tilting it moves gravity.
async fn mpu6050_part_reads(chip: Chip) {
    let Some(build) = esp32bench_sim::lcgamboa::Build::find() else {
        eprintln!("skipping {chip}: lcgamboa's QEMU is not built");
        return;
    };
    let image = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(format!("../../../tests/firmware/out/mpu6050-{chip}.bin"));
    if !image.is_file() {
        eprintln!("skipping {chip}: mpu6050 firmware not built");
        return;
    }
    let host = PathBuf::from(env!("CARGO_BIN_EXE_esp32bench-qemu-host"));
    let launch = esp32bench_sim::engine::lcgamboa_launch(chip, &build, Some(&host)).unwrap();
    let mut cfg = launch.config(chip, image);
    cfg.circuit = Some(
        r#"{"schemaVersion":"1.1.0","parts":[{"id":"imu","kind":"mpu6050","i2c":{"bus":0,"address":104}}]}"#
            .into(),
    );
    let mut s = Session::start(cfg).await.unwrap();
    let mut link = s.gpio.take().expect("no link to the host");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let mut text = String::new();
    let mut tilted = false;
    // 25 degC: 2500 or 2501 after the firmware's integer arithmetic.
    let flat = "ax=0 ay=0 az=16384 t=250";
    // pitch 30: ax = -sin 30 g, az = cos 30 g, at 16384 counts per g.
    let at_30 = "ax=-8192 ay=0 az=14189";
    while !(tilted && text.contains(at_30)) {
        tokio::select! {
            chunk = s.uart.read_chunk() => {
                if let Ok(Some(c)) = chunk { text.push_str(&String::from_utf8_lossy(&c)); }
                if !tilted && text.contains(flat) {
                    assert!(text.contains("esp32bench-mpu6050 ready who=0x68"), "{chip}: {text}");
                    link.control("imu", "pitch", &serde_json::json!(30.0)).await.unwrap();
                    tilted = true;
                }
            }
            ev = link.next_event() => {
                if let esp32bench_sim::gpio::PinEvent::PartError { id, message } = ev.unwrap().expect("host closed the link") {
                    panic!("{chip}: {id}: {message}");
                }
            }
            () = tokio::time::sleep_until(deadline) => panic!("{chip}: tilted={tilted}; uart:\n{text}"),
        }
    }
    s.shutdown().await.unwrap();
}

#[tokio::test]
async fn esp32_mpu6050_part_reads() {
    mpu6050_part_reads(Chip::Esp32).await;
}

#[tokio::test]
async fn esp32c3_mpu6050_part_reads() {
    mpu6050_part_reads(Chip::Esp32c3).await;
}

/// The DS3231 part model: tests/firmware/ds3231 sets the time to a second before
/// midnight on 2026-02-28 and arms alarm 1; the clock rolls into March, and the
/// host's tick fires the alarm and pulls SQW/INT low on a GPIO the firmware reads.
async fn ds3231_part_keeps_time(chip: Chip) {
    let Some(build) = esp32bench_sim::lcgamboa::Build::find() else {
        eprintln!("skipping {chip}: lcgamboa's QEMU is not built");
        return;
    };
    let image = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(format!("../../../tests/firmware/out/ds3231-{chip}.bin"));
    if !image.is_file() {
        eprintln!("skipping {chip}: ds3231 firmware not built");
        return;
    }
    let int_pin: u8 = if chip == Chip::Esp32c3 { 4 } else { 25 };
    let host = PathBuf::from(env!("CARGO_BIN_EXE_esp32bench-qemu-host"));
    let launch = esp32bench_sim::engine::lcgamboa_launch(chip, &build, Some(&host)).unwrap();
    let mut cfg = launch.config(chip, image);
    cfg.bridge = vec![int_pin];
    cfg.circuit = Some(format!(
        r#"{{"schemaVersion":"1.1.0","parts":[{{"id":"rtc","kind":"ds3231","pins":{{"sqw":{int_pin}}},"i2c":{{"bus":0,"address":104}}}}]}}"#
    ));
    let mut s = Session::start(cfg).await.unwrap();
    let mut link = s.gpio.take().expect("no link to the host");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let mut text = String::new();
    while !text.contains("esp32bench-ds3231 2026-03-01 00:00:0") || !text.contains("a1f=1 int=0") {
        tokio::select! {
            chunk = s.uart.read_chunk() => {
                if let Ok(Some(c)) = chunk { text.push_str(&String::from_utf8_lossy(&c)); }
            }
            ev = link.next_event() => {
                if let esp32bench_sim::gpio::PinEvent::PartError { id, message } = ev.unwrap().expect("host closed the link") {
                    panic!("{chip}: {id}: {message}");
                }
            }
            () = tokio::time::sleep_until(deadline) => panic!("{chip}: uart:\n{text}"),
        }
    }
    // Before midnight the alarm had not fired and INT was released. (A line can show
    // 23:59:59 with the flag set: the firmware reads the time and the flag in two
    // transactions, and the second can tick over between them, as on a real DS3231.)
    assert!(
        text.contains("2026-02-28 23:59:58 a1f=0 int=1"),
        "{chip}: {text}"
    );
    s.shutdown().await.unwrap();
}

#[tokio::test]
async fn esp32_ds3231_part_keeps_time() {
    ds3231_part_keeps_time(Chip::Esp32).await;
}

#[tokio::test]
async fn esp32c3_ds3231_part_keeps_time() {
    ds3231_part_keeps_time(Chip::Esp32c3).await;
}

/// WS2812 strips on RMT (patches 0007 and 0011): tests/firmware/arduino-periph sends
/// 192 symbols with rmtWrite (bit i is 1 unless i is a multiple of 3, so the bytes
/// run 0x6d 0xb6 0xdb) and one LED's worth with rgbLedWrite (red 0, green 0, blue 32).
/// The host finds each channel's pin through the GPIO matrix, decodes the frames and
/// gives them to strip parts on those pins.
async fn ws2812_parts_light(chip: Chip) {
    use esp32bench_sim::gpio::PinEvent;
    let Some(build) = esp32bench_sim::lcgamboa::Build::find() else {
        eprintln!("skipping {chip}: lcgamboa's QEMU is not built");
        return;
    };
    let image = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!(
        "../../../tests/firmware/out/arduino-periph-{chip}.bin"
    ));
    if !image.is_file() {
        eprintln!("skipping {chip}: arduino-periph firmware not built");
        return;
    }
    // The sketch's RMT and RGB pins.
    let (rmt_pin, rgb_pin) = if chip == Chip::Esp32c3 {
        (5, 6)
    } else {
        (16, 17)
    };
    let host = PathBuf::from(env!("CARGO_BIN_EXE_esp32bench-qemu-host"));
    let launch = esp32bench_sim::engine::lcgamboa_launch(chip, &build, Some(&host)).unwrap();
    let mut cfg = launch.config(chip, image);
    cfg.circuit = Some(format!(
        r#"{{"schemaVersion":"1.2.0","parts":[
            {{"id":"strip","kind":"ws2812","pin":{rmt_pin},"props":{{"count":2}}}},
            {{"id":"rgb","kind":"ws2812","pin":{rgb_pin},"props":{{"count":1}}}}]}}"#
    ));
    let mut s = Session::start(cfg).await.unwrap();
    let mut link = s.gpio.take().expect("no link to the host");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    let want_strip = serde_json::json!({ "leds": [[0xb6, 0x6d, 0xdb], [0xb6, 0x6d, 0xdb]] });
    let want_rgb = serde_json::json!({ "leds": [[0, 0, 32]] });
    let (mut strip, mut rgb) = (false, false);
    let mut text = Vec::new();
    while !(strip && rgb) {
        tokio::select! {
            chunk = s.uart.read_chunk() => {
                if let Ok(Some(c)) = chunk { text.extend_from_slice(&c) }
            }
            ev = link.next_event() => match ev.unwrap().expect("host closed the link") {
                PinEvent::PartError { id, message } => panic!("{chip}: {id}: {message}"),
                PinEvent::Part { id, state } => {
                    strip |= id == "strip" && state == want_strip;
                    rgb |= id == "rgb" && state == want_rgb;
                }
                _ => {}
            },
            () = tokio::time::sleep_until(deadline) => panic!(
                "{chip}: strip={strip} rgb={rgb}; uart:\n{}", String::from_utf8_lossy(&text)
            ),
        }
    }
    s.shutdown().await.unwrap();
}

#[tokio::test]
async fn esp32_ws2812_parts_light() {
    ws2812_parts_light(Chip::Esp32).await;
}

#[tokio::test]
async fn esp32c3_ws2812_parts_light() {
    ws2812_parts_light(Chip::Esp32c3).await;
}

/// The potentiometer part on an ADC pin: tests/firmware/adc reads ADC1 with ESP-IDF's
/// one-shot driver (as analogRead() does) and prints each new reading with ESP-IDF's
/// calibrated millivolts. The firmware reads back the wiper's voltage: half of 3.3 V
/// at the start, then a quarter, then full scale, which saturates the ADC.
async fn potentiometer_reads(chip: Chip) {
    let Some(build) = esp32bench_sim::lcgamboa::Build::find() else {
        eprintln!("skipping {chip}: lcgamboa's QEMU is not built");
        return;
    };
    let image = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(format!("../../../tests/firmware/out/adc-{chip}.bin"));
    if !image.is_file() {
        eprintln!("skipping {chip}: adc firmware not built");
        return;
    }
    // ADC1 channel 6 on the ESP32, channel 3 on the C3.
    let gpio = if chip == Chip::Esp32 { 34 } else { 3 };
    let host = PathBuf::from(env!("CARGO_BIN_EXE_esp32bench-qemu-host"));
    let launch = esp32bench_sim::engine::lcgamboa_launch(chip, &build, Some(&host)).unwrap();
    let mut cfg = launch.config(chip, image);
    cfg.circuit = Some(format!(
        r#"{{"schemaVersion":"1.2.0","parts":[{{"id":"pot","kind":"potentiometer","pin":{gpio}}}]}}"#
    ));
    let mut s = Session::start(cfg).await.unwrap();
    let mut link = s.gpio.take().expect("no link to the host");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let mut text = String::new();
    // (raw, millivolts) from the firmware's last reading line.
    let last = |text: &str| -> Option<(i64, i64)> {
        let l = text
            .lines()
            .rev()
            .find(|l| l.starts_with("esp32bench-adc raw="))?;
        let mut it = l.trim_end().split(['=', ' ']);
        let raw = it.nth(2)?.parse().ok()?;
        let mv = it.nth(1)?.parse().ok()?;
        Some((raw, mv))
    };
    // What each step should read (millivolts within 20, or saturated), and the slider
    // position that comes next.
    let mut steps = [
        (Some(1650), Some(25.0)),
        (Some(825), Some(100.0)),
        (None, None),
    ]
    .into_iter();
    let mut want = steps.next();
    while let Some((mv, next)) = want {
        tokio::select! {
            chunk = s.uart.read_chunk() => {
                if let Ok(Some(c)) = chunk { text.push_str(&String::from_utf8_lossy(&c)); }
                let reached = last(&text).is_some_and(|(raw, got)| match mv {
                    Some(mv) => (got - mv).abs() <= 20,
                    None => raw == 4095,
                });
                if reached {
                    assert!(text.contains(&format!("esp32bench-adc ready gpio={gpio}")), "{chip}: {text}");
                    if let Some(position) = next {
                        link.control("pot", "position", &serde_json::json!(position)).await.unwrap();
                    }
                    want = steps.next();
                }
            }
            ev = link.next_event() => {
                if let esp32bench_sim::gpio::PinEvent::PartError { id, message } = ev.unwrap().expect("host closed the link") {
                    panic!("{chip}: {id}: {message}");
                }
            }
            () = tokio::time::sleep_until(deadline) => panic!("{chip}: waiting for {mv:?} mV; uart:\n{text}"),
        }
    }
    s.shutdown().await.unwrap();
}

#[tokio::test]
async fn esp32_potentiometer_reads() {
    potentiometer_reads(Chip::Esp32).await;
}

#[tokio::test]
async fn esp32c3_potentiometer_reads() {
    potentiometer_reads(Chip::Esp32c3).await;
}

/// A potentiometer on a pin with no ADC says so, rather than reading 0 silently.
#[tokio::test]
async fn analog_part_on_a_digital_pin_says_so() {
    let Some(build) = esp32bench_sim::lcgamboa::Build::find() else {
        eprintln!("skipping: lcgamboa's QEMU is not built");
        return;
    };
    let image =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../tests/firmware/out/adc-esp32.bin");
    if !image.is_file() {
        eprintln!("skipping: adc firmware not built");
        return;
    }
    let host = PathBuf::from(env!("CARGO_BIN_EXE_esp32bench-qemu-host"));
    let launch = esp32bench_sim::engine::lcgamboa_launch(Chip::Esp32, &build, Some(&host)).unwrap();
    let mut cfg = launch.config(Chip::Esp32, image);
    cfg.circuit = Some(
        r#"{"schemaVersion":"1.2.0","parts":[{"id":"pot","kind":"potentiometer","pin":5}]}"#.into(),
    );
    let mut s = Session::start(cfg).await.unwrap();
    let mut link = s.gpio.take().expect("no link to the host");
    let message = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if let esp32bench_sim::gpio::PinEvent::PartError { id, message } = link
                .next_event()
                .await
                .unwrap()
                .expect("host closed the link")
            {
                assert_eq!(id, "pot");
                return message;
            }
        }
    })
    .await
    .expect("no error for GPIO5");
    assert!(message.contains("GPIO 5 has no analog input"), "{message}");
    s.shutdown().await.unwrap();
}
