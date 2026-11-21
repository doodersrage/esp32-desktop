//! The library projects the user starred, in a JSON file in the data directory
//! (`$XDG_DATA_HOME/esp32bench/starred.json`): a sorted list of project ids.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Starred {
    file: PathBuf,
}

impl Starred {
    pub fn new(file: PathBuf) -> Self {
        Starred { file }
    }

    pub fn path(&self) -> &Path {
        &self.file
    }

    /// The starred ids; none if the file is missing or unreadable.
    pub fn list(&self) -> Vec<String> {
        std::fs::read(&self.file)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    /// Star or unstar `id`; returns the starred ids after.
    pub fn set(&self, id: &str, starred: bool) -> std::io::Result<Vec<String>> {
        let mut all = self.list();
        all.retain(|x| x != id);
        if starred {
            all.push(id.to_owned());
        }
        all.sort();
        if let Some(dir) = self.file.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = self.file.with_extension("tmp");
        std::fs::write(
            &tmp,
            serde_json::to_vec_pretty(&all).map_err(std::io::Error::other)?,
        )?;
        std::fs::rename(tmp, &self.file)?;
        Ok(all)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stars_and_unstars() {
        let t = tempfile::tempdir().unwrap();
        let s = Starred::new(t.path().join("data/starred.json"));
        assert!(s.list().is_empty());
        assert_eq!(s.set("simon", true).unwrap(), ["simon"]);
        assert_eq!(s.set("blink", true).unwrap(), ["blink", "simon"]);
        assert_eq!(s.set("blink", true).unwrap(), ["blink", "simon"]);
        assert_eq!(s.set("simon", false).unwrap(), ["blink"]);
        assert_eq!(Starred::new(s.path().to_owned()).list(), ["blink"]);
    }
}
