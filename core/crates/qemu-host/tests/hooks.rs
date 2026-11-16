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
        match ev.as_str() {
            "out 2 0" => seen.0 = true,
            "out 2 1" => seen.1 = true,
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
    while !(events.contains(&PinEvent::Level {
        gpio: 2,
        level: true,
    }) && events.contains(&PinEvent::Level {
        gpio: 2,
        level: false,
    })) {
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
    // Drive the input and watch the firmware report it.
    gpio.set(4, true).await.unwrap();
    let found = s
        .uart
        .wait_for(b"esp32bench-gpio in 4=1", Duration::from_secs(10), |_| {})
        .await
        .unwrap();
    assert_eq!(found, WaitOutcome::Found);
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
