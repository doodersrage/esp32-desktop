//! Locating and installing the pinned QEMU build under the user's data directory.
//!
//! Layout: `$XDG_DATA_HOME/esp32bench/qemu/<release>/<arch>/qemu/bin/<binary>`.
//! `ESP32BENCH_QEMU_DIR` overrides the location for one arch-independent `bin/` directory.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use tokio::process::Command;

use crate::pin::{self, QEMU_RELEASE};
use crate::{Arch, Error};

pub const APP_DIR: &str = "esp32bench";
pub const QEMU_DIR_ENV: &str = "ESP32BENCH_QEMU_DIR";

pub fn data_dir() -> Result<PathBuf, Error> {
    if let Some(d) = std::env::var_os("XDG_DATA_HOME").filter(|d| !d.is_empty()) {
        return Ok(PathBuf::from(d).join(APP_DIR));
    }
    let home = std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .ok_or(Error::NoDataDir)?;
    Ok(PathBuf::from(home).join(".local/share").join(APP_DIR))
}

/// Install directory for one architecture of the pinned release.
pub fn arch_dir(arch: Arch) -> Result<PathBuf, Error> {
    Ok(data_dir()?
        .join("qemu")
        .join(QEMU_RELEASE)
        .join(arch.as_str()))
}

/// Where the QEMU binary for `arch` is expected, whether or not it exists.
pub fn qemu_binary_path(arch: Arch) -> Result<PathBuf, Error> {
    if let Some(d) = std::env::var_os(QEMU_DIR_ENV).filter(|d| !d.is_empty()) {
        return Ok(PathBuf::from(d).join(arch.binary_name()));
    }
    Ok(arch_dir(arch)?
        .join("qemu")
        .join("bin")
        .join(arch.binary_name()))
}

/// The QEMU binary for `arch`, or an error telling the user how to install it.
pub fn find_qemu(arch: Arch) -> Result<PathBuf, Error> {
    let path = qemu_binary_path(arch)?;
    if path.is_file() {
        Ok(path)
    } else {
        Err(Error::QemuNotInstalled { arch, path })
    }
}

/// Download, verify and unpack the pinned QEMU for `arch`. Returns the binary path.
///
/// Uses `curl` and `tar` with explicit argument lists. The archive is unpacked into a
/// staging directory and renamed into place, so a failed install leaves nothing behind.
pub async fn install_qemu(arch: Arch) -> Result<PathBuf, Error> {
    let asset = pin::asset(arch);
    let final_dir = arch_dir(arch)?;
    let parent = final_dir.parent().ok_or(Error::NoDataDir)?.to_path_buf();
    tokio::fs::create_dir_all(&parent).await?;

    let archive = parent.join(format!(".{arch}.tar.xz.partial"));
    let staging = parent.join(format!(".{arch}.partial"));
    let _cleanup = Cleanup(vec![archive.clone(), staging.clone()]);

    let out = Command::new("curl")
        .args(["--fail", "--location", "--silent", "--show-error"])
        .args(["--proto", "=https", "--proto-redir", "=https"])
        .arg("--output")
        .arg(&archive)
        .arg(asset.url)
        .output()
        .await?;
    if !out.status.success() {
        return Err(Error::Download {
            url: asset.url.to_owned(),
            detail: String::from_utf8_lossy(&out.stderr).trim().to_owned(),
        });
    }

    let actual = sha256_file(&archive).await?;
    if actual != asset.sha256 {
        return Err(Error::Checksum {
            url: asset.url.to_owned(),
            expected: asset.sha256.to_owned(),
            actual,
        });
    }

    let _ = tokio::fs::remove_dir_all(&staging).await;
    tokio::fs::create_dir_all(&staging).await?;
    let out = Command::new("tar")
        .args(["--extract", "--xz", "--no-same-owner", "--file"])
        .arg(&archive)
        .arg("--directory")
        .arg(&staging)
        .output()
        .await?;
    if !out.status.success() {
        return Err(Error::Extract {
            path: archive.clone(),
            detail: String::from_utf8_lossy(&out.stderr).trim().to_owned(),
        });
    }
    let bin = staging.join("qemu").join("bin").join(arch.binary_name());
    if !bin.is_file() {
        return Err(Error::Extract {
            path: archive.clone(),
            detail: format!("archive has no qemu/bin/{}", arch.binary_name()),
        });
    }

    let _ = tokio::fs::remove_dir_all(&final_dir).await;
    tokio::fs::rename(&staging, &final_dir).await?;
    Ok(final_dir.join("qemu").join("bin").join(arch.binary_name()))
}

pub async fn sha256_file(path: &Path) -> Result<String, Error> {
    let data = tokio::fs::read(path).await?;
    Ok(hex(&Sha256::digest(&data)))
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// Best-effort removal of temporary install files.
struct Cleanup(Vec<PathBuf>);

impl Drop for Cleanup {
    fn drop(&mut self) {
        for p in &self.0 {
            if p.is_dir() {
                let _ = std::fs::remove_dir_all(p);
            } else {
                let _ = std::fs::remove_file(p);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn hashes_files() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x");
        std::fs::write(&p, b"abc").unwrap();
        assert_eq!(
            sha256_file(&p).await.unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
