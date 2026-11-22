//! Where esp32bench keeps its files on each system. The XDG variables win where they
//! are set (tests point them at temporary directories); otherwise each system's own
//! convention:
//!
//! | | data | cache | Arduino's data (arduino-cli's own default) |
//! | --- | --- | --- | --- |
//! | Linux | `~/.local/share/esp32bench` | `~/.cache/esp32bench` | `~/.arduino15` |
//! | macOS | `~/Library/Application Support/esp32bench` | `~/Library/Caches/esp32bench` | `~/Library/Arduino15` |
//! | Windows | `%LOCALAPPDATA%\esp32bench` | `%LOCALAPPDATA%\esp32bench\cache` | `%LOCALAPPDATA%\Arduino15` |

use std::ffi::OsString;
use std::path::PathBuf;

pub const APP: &str = "esp32bench";

fn var(name: &str) -> Option<OsString> {
    std::env::var_os(name).filter(|v| !v.is_empty())
}

/// The user's home: `$HOME`, or on Windows `%USERPROFILE%`.
pub fn home() -> Option<PathBuf> {
    var("HOME")
        .or_else(|| var("USERPROFILE"))
        .map(PathBuf::from)
}

/// Windows' per-user, per-computer application data (`%LOCALAPPDATA%`).
fn local_app_data() -> Option<PathBuf> {
    var("LOCALAPPDATA")
        .map(PathBuf::from)
        .or_else(|| home().map(|h| h.join("AppData").join("Local")))
}

/// The app's data directory (built tools, the simulator, circuits, projects).
pub fn data_dir() -> Option<PathBuf> {
    if let Some(d) = var("XDG_DATA_HOME") {
        return Some(PathBuf::from(d).join(APP));
    }
    if cfg!(windows) {
        local_app_data().map(|d| d.join(APP))
    } else if cfg!(target_os = "macos") {
        home().map(|h| h.join("Library/Application Support").join(APP))
    } else {
        home().map(|h| h.join(".local/share").join(APP))
    }
}

/// The app's cache directory (the project library, build output).
pub fn cache_dir() -> Option<PathBuf> {
    if let Some(d) = var("XDG_CACHE_HOME") {
        return Some(PathBuf::from(d).join(APP));
    }
    if cfg!(windows) {
        local_app_data().map(|d| d.join(APP).join("cache"))
    } else if cfg!(target_os = "macos") {
        home().map(|h| h.join("Library/Caches").join(APP))
    } else {
        home().map(|h| h.join(".cache").join(APP))
    }
}

/// Arduino's data directory, where arduino-cli keeps cores and package indexes by
/// default: `$ARDUINO_DIRECTORIES_DATA` if set.
pub fn arduino_data() -> Option<PathBuf> {
    if let Some(d) = var("ARDUINO_DIRECTORIES_DATA") {
        return Some(PathBuf::from(d));
    }
    if cfg!(windows) {
        local_app_data().map(|d| d.join("Arduino15"))
    } else if cfg!(target_os = "macos") {
        home().map(|h| h.join("Library/Arduino15"))
    } else {
        home().map(|h| h.join(".arduino15"))
    }
}

/// The bytes of the files under `dir` (links are not followed); 0 if it is missing.
pub fn dir_bytes(dir: &std::path::Path) -> u64 {
    let mut total = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            match entry.file_type() {
                Ok(t) if t.is_dir() => stack.push(entry.path()),
                Ok(t) if t.is_file() => total += entry.metadata().map_or(0, |m| m.len()),
                _ => {}
            }
        }
    }
    total
}
