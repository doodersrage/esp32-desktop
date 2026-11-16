//! QEMU boot tests. They need the pinned QEMU (`esp32bench qemu install`) and a merged
//! image per chip. By default that is `tests/firmware/out/hello-<chip>.bin`, built by
//! `tests/firmware/build.sh`, and the test waits for its ready line. Set
//! `ESP32BENCH_TEST_IMAGE_<CHIP>` (for example `ESP32BENCH_TEST_IMAGE_ESP32C3=/path/flash.bin`)
//! to boot another image; the test then only waits for the ROM's `rst:0x`.
//! Without QEMU or an image, each test is skipped.

use std::path::PathBuf;
use std::time::Duration;

use esp32bench_sim::uart::WaitOutcome;
use esp32bench_sim::{Chip, Session, SessionConfig, gdb, install};

const HELLO_MARKER: &[u8] = b"esp32bench-hello ready";
const ROM_MARKER: &[u8] = b"rst:0x";

/// The image to boot and the UART0 line that proves it booted.
fn image_for(chip: Chip) -> Option<(PathBuf, &'static [u8])> {
    let var = format!(
        "ESP32BENCH_TEST_IMAGE_{}",
        chip.machine().to_ascii_uppercase()
    );
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
        machine: None,
        extra_args: Vec::new(),
        env: Vec::new(),
        bridge: Vec::new(),
    };
    Some((cfg, marker))
}

async fn boot_reset_and_quit(chip: Chip) {
    let Some((cfg, marker)) = setup(chip) else {
        return;
    };
    boot_reset_and_quit_with(cfg, marker).await;
}

/// The same on lcgamboa's fork, as `toolchain install qemu-lcgamboa` builds it.
async fn lcgamboa_boot_reset_and_quit(chip: Chip) {
    let Some(build) = esp32bench_sim::lcgamboa::Build::find() else {
        eprintln!("skipping {chip}: lcgamboa's QEMU is not built");
        return;
    };
    let Some((image, marker)) = image_for(chip) else {
        return;
    };
    let cfg = SessionConfig {
        chip,
        qemu: build.executable(chip.arch()),
        image,
        gdb_port: None,
        start_paused: false,
        machine: Some(esp32bench_sim::lcgamboa::machine(chip).to_owned()),
        extra_args: build.qemu_args(),
        env: Vec::new(),
        bridge: Vec::new(),
    };
    boot_reset_and_quit_with(cfg, marker).await;
}

#[tokio::test]
async fn lcgamboa_esp32_boots() {
    lcgamboa_boot_reset_and_quit(Chip::Esp32).await;
}

#[tokio::test]
async fn lcgamboa_esp32s3_boots() {
    lcgamboa_boot_reset_and_quit(Chip::Esp32s3).await;
}

#[tokio::test]
async fn lcgamboa_esp32c3_boots() {
    lcgamboa_boot_reset_and_quit(Chip::Esp32c3).await;
}

async fn boot_reset_and_quit_with(cfg: SessionConfig, marker: &[u8]) {
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

/// A built firmware image from tests/firmware/out, if present.
fn built_image(project: &str, chip: Chip) -> Option<PathBuf> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../tests/firmware/out")
        .join(format!("{project}-{chip}.bin"));
    if p.is_file() {
        Some(p)
    } else {
        eprintln!("skipping {project} on {chip}: {} not built", p.display());
        None
    }
}

/// The Arduino sketch, built with the pinned ESP32 Arduino core, boots and resets.
#[tokio::test]
async fn arduino_hello_boots() {
    const READY: &[u8] = b"esp32bench-arduino ready";
    for chip in Chip::ALL {
        let Some(image) = built_image("hello-arduino", chip) else {
            continue;
        };
        let Ok(qemu) = install::find_qemu(chip.arch()) else {
            eprintln!("skipping hello-arduino on {chip}: QEMU not installed");
            continue;
        };
        let cfg = SessionConfig {
            chip,
            qemu,
            image,
            gdb_port: None,
            start_paused: false,
            machine: None,
            extra_args: Vec::new(),
            env: Vec::new(),
            bridge: Vec::new(),
        };
        let mut s = Session::start(cfg).await.unwrap();
        let t = Duration::from_secs(30);
        assert_eq!(
            s.uart.wait_for(READY, t, |_| {}).await.unwrap(),
            WaitOutcome::Found,
            "{chip}"
        );
        s.reset().await.unwrap();
        assert_eq!(
            s.uart.wait_for(READY, t, |_| {}).await.unwrap(),
            WaitOutcome::Found,
            "{chip} after reset"
        );
        s.shutdown().await.unwrap();
    }
}

/// The hello firmware echoes lines it receives on UART0.
#[tokio::test]
async fn uart_input_reaches_the_firmware() {
    for chip in Chip::ALL {
        let Some((cfg, marker)) = setup(chip) else {
            continue;
        };
        if marker != HELLO_MARKER {
            eprintln!("skipping UART input on {chip}: needs the hello firmware");
            continue;
        }
        let mut s = Session::start(cfg).await.unwrap();
        let t = Duration::from_secs(30);
        assert_eq!(
            s.uart.wait_for(marker, t, |_| {}).await.unwrap(),
            WaitOutcome::Found
        );
        s.uart.write(b"ping 42\r\n").await.unwrap();
        assert_eq!(
            s.uart
                .wait_for(b"esp32bench-hello echo ping 42", t, |_| {})
                .await
                .unwrap(),
            WaitOutcome::Found,
            "{chip} did not echo"
        );
        s.shutdown().await.unwrap();
    }
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
