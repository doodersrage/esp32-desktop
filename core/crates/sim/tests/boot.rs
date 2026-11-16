//! QEMU boot tests. They need the pinned QEMU (`esp32d qemu install`) and a merged
//! image per chip, given as `ESP32D_TEST_IMAGE_<CHIP>` (for example
//! `ESP32D_TEST_IMAGE_ESP32C3=/path/flash.bin`). Without them each test is skipped.

use std::time::Duration;

use esp32d_sim::uart::WaitOutcome;
use esp32d_sim::{Chip, Session, SessionConfig, gdb, install};

const MARKER: &[u8] = b"rst:0x";

fn setup(chip: Chip) -> Option<SessionConfig> {
    let var = format!("ESP32D_TEST_IMAGE_{}", chip.machine().to_ascii_uppercase());
    let Some(image) = std::env::var_os(&var) else {
        eprintln!("skipping {chip}: {var} not set");
        return None;
    };
    let qemu = match install::find_qemu(chip.arch()) {
        Ok(q) => q,
        Err(e) => {
            eprintln!("skipping {chip}: {e}");
            return None;
        }
    };
    Some(SessionConfig {
        chip,
        qemu,
        image: image.into(),
        gdb_port: None,
        start_paused: false,
    })
}

async fn boot_reset_and_quit(chip: Chip) {
    let Some(cfg) = setup(chip) else { return };
    let dir = {
        let mut s = Session::start(cfg).await.unwrap();
        let dir = s.paths().dir.clone();
        let t = Duration::from_secs(30);
        assert_eq!(
            s.uart.wait_for(MARKER, t, |_| {}).await.unwrap(),
            WaitOutcome::Found
        );
        s.reset().await.unwrap();
        assert_eq!(
            s.uart.wait_for(MARKER, t, |_| {}).await.unwrap(),
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
    let Some(mut cfg) = setup(Chip::Esp32) else {
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
