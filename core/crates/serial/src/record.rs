//! Recording a port to a file: every byte the board sends, as it sends it, appended to
//! a file the user chose (serial.record), for logs longer than the app's console keeps.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

pub struct Recording {
    file: File,
    path: PathBuf,
    bytes: u64,
}

impl Recording {
    /// Append to `path`, creating it. It must be an absolute path to a file.
    pub fn start(path: &Path) -> std::io::Result<Self> {
        if !path.is_absolute() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "the recording needs an absolute path",
            ));
        }
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        Ok(Recording {
            file,
            path: path.to_owned(),
            bytes: 0,
        })
    }

    /// Write a chunk as it came; each chunk goes to the file at once, so a crash or a
    /// pulled cable loses nothing already received.
    pub fn write(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.file.write_all(bytes)?;
        self.bytes += bytes.len() as u64;
        Ok(())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Bytes written since it started.
    pub fn bytes(&self) -> u64 {
        self.bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_what_comes() {
        let t = tempfile::tempdir().unwrap();
        let path = t.path().join("board.log");
        std::fs::write(&path, b"before\n").unwrap();
        let mut r = Recording::start(&path).unwrap();
        r.write(b"rst:0x1\n").unwrap();
        r.write(&[0xff, b'\n']).unwrap();
        assert_eq!(r.bytes(), 10);
        assert_eq!(r.path(), path);
        assert_eq!(std::fs::read(&path).unwrap(), b"before\nrst:0x1\n\xff\n");
        assert!(Recording::start(Path::new("relative.log")).is_err());
        assert!(Recording::start(t.path()).is_err()); // a folder
    }
}
