//! espflash against QEMU in download mode: the emulated ESP32 runs its real ROM
//! bootloader on a pseudo-terminal, so connecting, reading the partition table and
//! writing flash are exercised without hardware. Skipped without the pinned QEMU or
//! the hello firmware (tests/firmware/build.sh).

use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use esp32bench_flash::{Board, Options, Reset};

struct Qemu(Child);

impl Drop for Qemu {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Start QEMU with `flash` in download mode and return it and its UART pty.
fn qemu_download_mode(flash: &Path, log: &Path) -> Option<(Qemu, PathBuf)> {
    let qemu = esp32bench_sim::install::find_qemu(esp32bench_sim::Arch::Xtensa).ok()?;
    let stderr = std::fs::File::create(log).unwrap();
    let mut drive = std::ffi::OsString::from("file=");
    drive.push(flash);
    drive.push(",if=mtd,format=raw");
    let child = Command::new(qemu)
        .args(["-machine", "esp32", "-display", "none", "-monitor", "none"])
        .arg("-drive")
        .arg(drive)
        // GPIO strapping for "boot from UART": the ROM waits for a download.
        .args([
            "-global",
            "driver=esp32.gpio,property=strap_mode,value=0x0f",
        ])
        .args(["-serial", "pty"])
        // QEMU announces the pty on stdout; keep both streams in the log.
        .stdout(stderr.try_clone().unwrap())
        .stderr(stderr)
        .spawn()
        .unwrap();
    let q = Qemu(child);
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        let text = std::fs::read_to_string(log).unwrap_or_default();
        if let Some(i) = text.find("/dev/pts/") {
            let pty: String = text[i..]
                .chars()
                .take_while(|c| !c.is_whitespace() && *c != ')')
                .collect();
            return Some((q, PathBuf::from(pty)));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!(
        "QEMU did not report its pty: {}",
        std::fs::read_to_string(log).unwrap_or_default()
    );
}

#[test]
fn reads_and_writes_an_emulated_esp32() {
    let hello = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../tests/firmware/out/hello-esp32.bin");
    if !hello.is_file() {
        eprintln!("skipping: {} not built", hello.display());
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let flash = dir.path().join("flash.bin");
    std::fs::copy(&hello, &flash).unwrap();
    let Some((qemu, pty)) = qemu_download_mode(&flash, &dir.path().join("qemu.log")) else {
        eprintln!("skipping: QEMU is not installed");
        return;
    };

    // The stub does not run in QEMU; the ROM loader does everything.
    let mut board = Board::connect(&Options {
        port: pty,
        reset: Reset::None,
        use_stub: false,
    })
    .unwrap();
    let info = board.info().unwrap();
    assert_eq!(info.chip, "esp32");
    assert_eq!(info.crystal_mhz, 40);
    assert_eq!(info.flash_bytes, 4 * 1024 * 1024);

    // The hello firmware's IDF default table: nvs, phy_init, factory app.
    let parts = board.partitions(dir.path()).unwrap();
    let factory = parts
        .iter()
        .find(|p| p.subtype == "factory")
        .expect("a factory app partition");
    assert_eq!(factory.kind, "app");
    assert_eq!(factory.offset, 0x10000);
    assert!(parts.iter().any(|p| p.subtype == "nvs"));

    // Write a pattern (espflash verifies it with the chip's MD5), then check the
    // emulated flash really holds it.
    let data: Vec<u8> = (0..8192u32).map(|i| (i * 7 % 251) as u8).collect();
    let mut steps = Vec::new();
    board
        .write(0x300000, &data, &mut |p| steps.push(p))
        .unwrap();
    assert!(!steps.is_empty());
    drop(board);
    drop(qemu);
    let on_disk = std::fs::read(&flash).unwrap();
    assert_eq!(
        &on_disk[0x300000..0x300000 + data.len()],
        &data[..],
        "bytes did not reach the flash"
    );
}
