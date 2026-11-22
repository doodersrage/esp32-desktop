//! Crash backtraces: the addresses a panicking ESP32 prints, found in its output, and
//! turned into functions, files and lines from the build's ELF with the toolchain's
//! `addr2line`, as the Arduino IDE's exception decoder and `idf.py monitor` do.
//!
//! Xtensa chips (ESP32, S3) print `Backtrace: 0x400d2a5e:0x3ffb2270 0x400d1b6f:…`,
//! program counter and stack pointer pairs; RISC-V chips (C3) print a register dump
//! whose `MEPC` (where it stopped) and `RA` (where it was called from) are used.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::LazyLock;
use std::time::Duration;

use regex::Regex;
use serde::Serialize;
use tokio::process::Command;

/// At most this many addresses of one backtrace are decoded.
pub const MAX_ADDRESSES: usize = 32;

/// The toolchain release that the pinned Arduino ESP32 core (3.3.12) installs, whose
/// `addr2line` decodes.
pub const ARDUINO_TOOLCHAIN_VERSION: &str = "2601";

static XTENSA: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"Backtrace:((?:\s+0x[0-9a-fA-F]{8}:0x[0-9a-fA-F]{8})+)").expect("valid regex")
});
static PAIR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"0x([0-9a-fA-F]{8}):0x[0-9a-fA-F]{8}").expect("valid regex"));
static RISCV: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"MEPC\s*:\s*0x([0-9a-fA-F]{8})\s+RA\s*:\s*0x([0-9a-fA-F]{8})").expect("valid regex")
});

/// The code addresses of a backtrace line, innermost first; None if the line is not one.
pub fn addresses(line: &str) -> Option<Vec<u32>> {
    let found: Vec<u32> = if let Some(c) = XTENSA.captures(line) {
        PAIR.captures_iter(&c[1])
            .filter_map(|p| u32::from_str_radix(&p[1], 16).ok())
            .collect()
    } else {
        let c = RISCV.captures(line)?;
        [&c[1], &c[2]]
            .iter()
            .filter_map(|a| u32::from_str_radix(a, 16).ok())
            .collect()
    };
    let found: Vec<u32> = found
        .into_iter()
        .filter(|&a| a != 0)
        .take(MAX_ADDRESSES)
        .collect();
    (!found.is_empty()).then_some(found)
}

/// Finds backtraces in output as it comes, across chunks.
#[derive(Default)]
pub struct Finder {
    line: Vec<u8>,
}

/// Lines longer than this are not backtraces worth waiting for; they are dropped.
const MAX_LINE: usize = 4096;

impl Finder {
    pub fn new() -> Self {
        Self::default()
    }

    /// The backtraces among the lines this chunk finishes.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Vec<u32>> {
        let mut out = Vec::new();
        for &b in bytes {
            if b == b'\n' {
                if let Some(a) = addresses(&String::from_utf8_lossy(&self.line)) {
                    out.push(a);
                }
                self.line.clear();
            } else if self.line.len() < MAX_LINE {
                self.line.push(b);
            }
        }
        out
    }
}

/// One decoded address. An address inside inlined code gives several frames with the
/// same address: the inlined function first, then the ones it was inlined into.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Frame {
    pub addr: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub func: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
}

/// `addr2line -a -f -i -C` output as frames.
pub fn parse_addr2line(out: &str) -> Vec<Frame> {
    let mut frames = Vec::new();
    let mut addr: Option<String> = None;
    let mut lines = out.lines().map(str::trim).filter(|l| !l.is_empty());
    while let Some(l) = lines.next() {
        if l.starts_with("0x") && l[2..].chars().all(|c| c.is_ascii_hexdigit()) {
            addr = Some(format!(
                "0x{:08x}",
                u32::from_str_radix(&l[2..], 16).unwrap_or(0)
            ));
            continue;
        }
        let Some(a) = addr.clone() else { continue };
        let func = Some(l).filter(|f| *f != "??").map(str::to_owned);
        let place = lines.next().unwrap_or("??:0");
        // "file:line", maybe with " (discriminator N)".
        let place = place.split(" (").next().unwrap_or(place);
        let (file, line) = match place.rsplit_once(':') {
            Some((f, n)) => (f, n.parse::<u32>().ok().filter(|&n| n > 0)),
            None => (place, None),
        };
        frames.push(Frame {
            addr: a,
            func,
            file: Some(file).filter(|f| *f != "??").map(str::to_owned),
            line,
        });
    }
    frames
}

/// The toolchain's `addr2line` for `chip` ("esp32", "esp32s3", "esp32c3") in an Arduino
/// data directory, if the pinned core installed it.
pub fn find_addr2line(arduino_data: &Path, chip: &str) -> Option<PathBuf> {
    let (tool, exe) = match chip {
        "esp32" => ("esp-x32", "xtensa-esp32-elf-addr2line"),
        "esp32s3" => ("esp-x32", "xtensa-esp32s3-elf-addr2line"),
        "esp32c3" => ("esp-rv32", "riscv32-esp-elf-addr2line"),
        _ => return None,
    };
    let path = arduino_data
        .join("packages/esp32/tools")
        .join(tool)
        .join(ARDUINO_TOOLCHAIN_VERSION)
        .join("bin")
        .join(format!("{exe}{}", std::env::consts::EXE_SUFFIX));
    path.is_file().then_some(path)
}

#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    #[error("could not run addr2line: {0}")]
    Io(#[from] std::io::Error),
    #[error("addr2line took too long")]
    Timeout,
    #[error("addr2line failed: {0}")]
    Failed(String),
}

/// Decode `addresses` with `addr2line` against `elf`.
pub async fn decode(
    addr2line: &Path,
    elf: &Path,
    addresses: &[u32],
) -> Result<Vec<Frame>, DecodeError> {
    let mut cmd = Command::new(addr2line);
    cmd.args(["-a", "-f", "-i", "-C", "-e"])
        .arg(elf)
        .args(
            addresses
                .iter()
                .take(MAX_ADDRESSES)
                .map(|a| format!("0x{a:08x}")),
        )
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let out = tokio::time::timeout(Duration::from_secs(10), cmd.output())
        .await
        .map_err(|_| DecodeError::Timeout)??;
    if !out.status.success() {
        return Err(DecodeError::Failed(
            String::from_utf8_lossy(&out.stderr).trim().to_owned(),
        ));
    }
    Ok(parse_addr2line(&String::from_utf8_lossy(&out.stdout)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_xtensa_backtraces() {
        let l = "\x1b[0;31mBacktrace: 0x400d2a5e:0x3ffb2270 0x400d1b6f:0x3ffb2290 0x00000000:0x3ffb22b0 |<-CORRUPTED";
        assert_eq!(addresses(l), Some(vec![0x400d2a5e, 0x400d1b6f]));
        assert_eq!(addresses("Backtrace: nothing here"), None);
        assert_eq!(addresses("hello"), None);
    }

    #[test]
    fn finds_riscv_register_dumps() {
        let l = "MEPC    : 0x42000d8e  RA      : 0x42000d8a  SP      : 0x3fc8fcb0  GP      : 0x3fc8b200";
        assert_eq!(addresses(l), Some(vec![0x42000d8e, 0x42000d8a]));
    }

    #[test]
    fn finds_across_chunks() {
        let mut f = Finder::new();
        assert!(f.feed(b"Guru\r\nBacktrace: 0x400d2a5e:0x3ff").is_empty());
        assert_eq!(f.feed(b"b2270\r\nrebooting\n"), vec![vec![0x400d2a5e]]);
    }

    #[test]
    fn parses_addr2line_with_inlining() {
        let out = "0x400d2a5e\ncrash()\n/s/blink/blink.ino:7\nloop()\n/s/blink/blink.ino:12 (discriminator 2)\n0x400d1b6f\nloopTask(void*)\n/c/main.cpp:74\n0x40001234\n??\n??:0\n";
        let f = parse_addr2line(out);
        assert_eq!(f.len(), 4);
        assert_eq!(f[0].func.as_deref(), Some("crash()"));
        assert_eq!(f[1].addr, "0x400d2a5e");
        assert_eq!(f[1].line, Some(12));
        assert_eq!(f[1].file.as_deref(), Some("/s/blink/blink.ino"));
        assert_eq!(f[2].func.as_deref(), Some("loopTask(void*)"));
        assert_eq!(
            f[3],
            Frame {
                addr: "0x40001234".into(),
                func: None,
                file: None,
                line: None
            }
        );
    }
}
