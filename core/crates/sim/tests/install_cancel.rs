//! A cancelled install (the daemon's job.cancel drops its future) stops its download
//! and leaves no partial files. Its own test binary: it puts a fake curl first on PATH.

#![cfg(target_os = "linux")]

use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

#[tokio::test]
async fn cancelling_stops_the_download_and_cleans_up() {
    let t = tempfile::tempdir().unwrap();
    let bin = t.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let pid_file = t.path().join("curl.pid");
    // A download that never finishes: it starts writing the archive, then waits.
    let curl = bin.join("curl");
    std::fs::write(
        &curl,
        format!(
            "#!/bin/sh\necho $$ > '{}'\nwhile [ \"$1\" != --output ]; do shift; done\nprintf partial > \"$2\"\nexec sleep 30\n",
            pid_file.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&curl, std::fs::Permissions::from_mode(0o755)).unwrap();
    // SAFETY: this test binary runs this one test; nothing else reads PATH meanwhile.
    unsafe {
        std::env::set_var(
            "PATH",
            format!(
                "{}:{}",
                bin.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        );
    }
    let data = t.path().join("data");
    let target = data.join("qemu");
    let install = esp32bench_sim::install::install_archive(
        "https://example.invalid/qemu.tar.xz",
        "00",
        &target,
        std::path::Path::new("bin/qemu"),
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(800), install)
            .await
            .is_err()
    );
    let pid: u32 = std::fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    // The download is stopped...
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::path::Path::new(&format!("/proc/{pid}")).exists() {
        let state = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
        if state.split_whitespace().nth(2) == Some("Z") {
            break; // exited, waiting to be reaped
        }
        assert!(
            std::time::Instant::now() < deadline,
            "curl {pid} still runs"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    // ...and nothing partial is left.
    let left: Vec<_> = std::fs::read_dir(&data)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert!(left.is_empty(), "{left:?}");
}
