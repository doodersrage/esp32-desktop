//! QEMU boot tests. They need the pinned QEMU (`esp32d qemu install`) and a merged
//! image per chip. By default that is `tests/firmware/out/hello-<chip>.bin`, built by
//! `tests/firmware/build.sh`, and the test waits for its ready line. Set
//! `ESP32D_TEST_IMAGE_<CHIP>` (for example `ESP32D_TEST_IMAGE_ESP32C3=/path/flash.bin`)
//! to boot another image; the test then only waits for the ROM's `rst:0x`.
//! Without QEMU or an image, each test is skipped.

use std::path::PathBuf;
use std::time::Duration;

use esp32d_sim::uart::WaitOutcome;
use esp32d_sim::{Chip, Session, SessionConfig, gdb, install};

const HELLO_MARKER: &[u8] = b"esp32d-hello ready";
const ROM_MARKER: &[u8] = b"rst:0x";

/// The image to boot and the UART0 line that proves it booted.
fn image_for(chip: Chip) -> Option<(PathBuf, &'static [u8])> {
    let var = format!("ESP32D_TEST_IMAGE_{}", chip.machine().to_ascii_uppercase());
    if let Some(image) = std::env::var_os(&var) {
        return Some((image.into(), ROM_MARKER));
    }
    let hello = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../tests/firmware/out")
        .join(format!("hello-{chip}.bin"));
    if hello.is_file() {
        return Some((hello, HELLO_MARKER));
    }
    eprintln!(
        "skipping {chip}: no {} (run tests/firmware/build.sh) and {var} not set",
        hello.display()
    );
    None
}

fn setup(chip: Chip) -> Option<(SessionConfig, &'static [u8])> {
    let (image, marker) = image_for(chip)?;
    let qemu = match install::find_qemu(chip.arch()) {
        Ok(q) => q,
        Err(e) => {
            eprintln!("skipping {chip}: {e}");
            return None;
        }
    };
    let cfg = SessionConfig {
        chip,
        qemu,
        image,
        gdb_port: None,
        start_paused: false,
    };
    Some((cfg, marker))
}

async fn boot_reset_and_quit(chip: Chip) {
    let Some((cfg, marker)) = setup(chip) else {
        return;
    };
    let dir = {
        let mut s = Session::start(cfg).await.unwrap();
        let dir = s.paths().dir.clone();
        let t = Duration::from_secs(30);
        assert_eq!(
            s.uart.wait_for(marker, t, |_| {}).await.unwrap(),
            WaitOutcome::Found
        );
        s.reset().await.unwrap();
        assert_eq!(
            s.uart.wait_for(marker, t, |_| {}).await.unwrap(),
            WaitOutcome::Found
        );
        s.shutdown().await.unwrap();
        dir
    };
    assert!(!dir.exists(), "session directory left behind");
}

#[tokio::test]
async fn esp32_boots() {
    boot_reset_and_quit(Chip::Esp32).await;
}

#[tokio::test]
async fn esp32s3_boots() {
    boot_reset_and_quit(Chip::Esp32s3).await;
}

#[tokio::test]
async fn esp32c3_boots() {
    boot_reset_and_quit(Chip::Esp32c3).await;
}

#[tokio::test]
async fn gdb_stub_answers_while_halted() {
    let Some((mut cfg, _)) = setup(Chip::Esp32) else {
        return;
    };
    cfg.gdb_port = Some(41234);
    cfg.start_paused = true;
    let s = Session::start(cfg).await.unwrap();
    let reply = gdb::probe(s.gdb_addr().unwrap(), Duration::from_secs(5))
        .await
        .unwrap();
    assert!(
        reply.starts_with('T') || reply.starts_with('S'),
        "stop reply: {reply}"
    );
    s.shutdown().await.unwrap();
}
