use std::path::PathBuf;

use crate::{Arch, Chip};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("unknown chip `{0}`")]
    UnknownChip(String),
    #[error("`{0}` is workbench-only: the simulator supports ESP32, ESP32-S3 and ESP32-C3")]
    NotSimulated(String),
    #[error("unknown QEMU architecture `{0}` (expected xtensa or riscv32)")]
    UnknownArch(String),

    #[error("not a merged {chip} flash image: no bootloader header at offset {offset:#x}")]
    NotMergedImage { chip: Chip, offset: usize },
    #[error("image is {0} bytes; the largest flash QEMU emulates is 16 MiB")]
    ImageTooLarge(usize),

    #[error("QEMU for {arch} is not installed (looked for {path}); run `esp32d qemu install`")]
    QemuNotInstalled { arch: Arch, path: PathBuf },
    #[error("no home or data directory: set XDG_DATA_HOME or HOME")]
    NoDataDir,
    #[error("download of {url} failed: {detail}")]
    Download { url: String, detail: String },
    #[error("checksum mismatch for {url}: expected {expected}, got {actual}")]
    Checksum {
        url: String,
        expected: String,
        actual: String,
    },
    #[error("extracting {path} failed: {detail}")]
    Extract { path: PathBuf, detail: String },

    #[error("QEMU exited before it was ready ({status}): {stderr}")]
    QemuExited { status: String, stderr: String },
    #[error("timed out waiting for {0}")]
    Timeout(&'static str),
    #[error("QMP protocol error: {0}")]
    QmpProtocol(String),
    #[error("QMP command `{command}` failed: {class}: {desc}")]
    QmpCommand {
        command: String,
        class: String,
        desc: String,
    },
    #[error("GDB remote protocol error: {0}")]
    GdbProtocol(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}
