//! GPIO round trip through lcgamboa's QEMU library: the host sees the firmware's
//! output pin toggle, and the firmware sees an input pin the host drives.
//!
//! Needs a library build of the fork (`build_libqemu-esp32.sh`) and the gpio test
//! firmware (`tests/firmware/build.sh gpio`). Set `ESP32D_TEST_QEMU_LIB_XTENSA`
//! and/or `ESP32D_TEST_QEMU_LIB_RISCV32` to `<fork>/build/libqemu-*.so`; the ROMs
//! are taken from `<fork>/pc-bios`. Without them the test is skipped.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use esp32d_sim::uart::WaitOutcome;
use esp32d_sim::{Chip, Session, SessionConfig};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::mpsc;

async fn round_trip(chip: Chip, machine: &str, lib_var: &str) {
    let Some(lib) = std::env::var_os(lib_var).map(PathBuf::from) else {
        eprintln!("skipping {chip}: {lib_var} not set");
        return;
    };
    let image = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../tests/firmware/out")
        .join(format!("gpio-{chip}.bin"));
    if !image.is_file() {
        eprintln!("skipping {chip}: {} not built", image.display());
        return;
    }
    let pc_bios = lib
        .parent()
        .and_then(Path::parent)
        .map(|tree| tree.join("pc-bios"))
        .unwrap();
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("esp32-desktop");
    std::fs::create_dir_all(&runtime).unwrap();
    let sock = runtime.join(format!("gpio-test-{}-{chip}.sock", std::process::id()));
    let _ = std::fs::remove_file(&sock);

    let cfg = SessionConfig {
        chip,
        qemu: env!("CARGO_BIN_EXE_esp32d-qemu-host").into(),
        image,
        gdb_port: None,
        start_paused: false,
        machine: Some(machine.into()),
        extra_args: vec!["-L".into(), pc_bios.into()],
        env: vec![
            ("ESP32D_QEMU_LIB".into(), lib.into()),
            ("ESP32D_GPIO_PINS".into(), "2,4".into()),
            (OsString::from("ESP32D_GPIO_SOCK"), sock.clone().into()),
        ],
    };
    let mut s = Session::start(cfg).await.unwrap();

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
        .wait_for(b"esp32d-gpio ready", t, |_| {})
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
        (&b"set 4 1\n"[..], &b"esp32d-gpio in 4=1"[..]),
        (&b"set 4 0\n"[..], &b"esp32d-gpio in 4=0"[..]),
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
        "ESP32D_TEST_QEMU_LIB_XTENSA",
    )
    .await;
}

#[tokio::test]
async fn esp32c3_gpio_round_trip() {
    round_trip(
        Chip::Esp32c3,
        "esp32c3-picsimlab",
        "ESP32D_TEST_QEMU_LIB_RISCV32",
    )
    .await;
}
