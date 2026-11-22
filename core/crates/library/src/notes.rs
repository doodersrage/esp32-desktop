//! The user's own notes per project (wiring changes, what worked, what to try next),
//! in a JSON file in the data directory (`notes.json`): project id to text.

use std::collections::BTreeMap;
use std::path::PathBuf;

/// The longest note kept, in characters.
pub const MAX_LENGTH: usize = 20_000;

#[derive(Debug, Clone)]
pub struct Notes {
    file: PathBuf,
}

impl Notes {
    pub fn new(file: PathBuf) -> Self {
        Notes { file }
    }

    /// Every project's note.
    pub fn all(&self) -> BTreeMap<String, String> {
        std::fs::read(&self.file)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    /// A project's note; empty if it has none.
    pub fn get(&self, project: &str) -> String {
        self.all().remove(project).unwrap_or_default()
    }

    /// Keep `text` as the project's note (cut to [`MAX_LENGTH`]); empty text forgets it.
    /// The note kept.
    pub fn set(&self, project: &str, text: &str) -> std::io::Result<String> {
        let mut all = self.all();
        let text: String = text.chars().take(MAX_LENGTH).collect();
        if text.trim().is_empty() {
            all.remove(project);
        } else {
            all.insert(project.to_owned(), text.clone());
        }
        if let Some(dir) = self.file.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = self.file.with_extension("tmp");
        std::fs::write(
            &tmp,
            serde_json::to_vec_pretty(&all).map_err(std::io::Error::other)?,
        )?;
        std::fs::rename(tmp, &self.file)?;
        Ok(if text.trim().is_empty() {
            String::new()
        } else {
            text
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_a_note_per_project() {
        let t = tempfile::tempdir().unwrap();
        let notes = Notes::new(t.path().join("data/notes.json"));
        assert_eq!(notes.get("blink"), "");
        assert_eq!(
            notes.set("blink", "LED on GPIO 4 now").unwrap(),
            "LED on GPIO 4 now"
        );
        notes.set("oled", "address 0x3D").unwrap();
        assert_eq!(notes.get("blink"), "LED on GPIO 4 now");
        assert_eq!(notes.get("oled"), "address 0x3D");
        assert_eq!(notes.set("blink", "  ").unwrap(), "");
        assert_eq!(notes.get("blink"), "");
        let long = "x".repeat(MAX_LENGTH + 10);
        assert_eq!(
            notes.set("oled", &long).unwrap().chars().count(),
            MAX_LENGTH
        );
    }
}
