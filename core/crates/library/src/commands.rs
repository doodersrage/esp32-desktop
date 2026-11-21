//! Lines the user sends to boards often, kept per project for the serial monitor's
//! quick-send buttons, in a JSON file in the data directory (`commands.json`): an
//! object from project id ("" for none) to its lines.

use std::collections::BTreeMap;
use std::path::PathBuf;

/// Most lines kept for one project, and the longest line.
pub const MAX_COMMANDS: usize = 20;
pub const MAX_LENGTH: usize = 200;

#[derive(Debug, Clone)]
pub struct Commands {
    file: PathBuf,
}

impl Commands {
    pub fn new(file: PathBuf) -> Self {
        Commands { file }
    }

    fn all(&self) -> BTreeMap<String, Vec<String>> {
        std::fs::read(&self.file)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    /// The lines kept for `project`.
    pub fn get(&self, project: &str) -> Vec<String> {
        self.all().remove(project).unwrap_or_default()
    }

    /// Keep `lines` for `project` (an empty list forgets them); blank lines are
    /// dropped, and so are repeats. Returns what is kept.
    pub fn set(&self, project: &str, lines: Vec<String>) -> std::io::Result<Vec<String>> {
        let mut kept: Vec<String> = Vec::new();
        for l in lines {
            if !l.trim().is_empty() && !kept.contains(&l) && kept.len() < MAX_COMMANDS {
                kept.push(l.chars().take(MAX_LENGTH).collect());
            }
        }
        let mut all = self.all();
        if kept.is_empty() {
            all.remove(project);
        } else {
            all.insert(project.to_owned(), kept.clone());
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
        Ok(kept)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_lines_per_project() {
        let t = tempfile::tempdir().unwrap();
        let c = Commands::new(t.path().join("data/commands.json"));
        assert!(c.get("simon").is_empty());
        let kept = c
            .set(
                "simon",
                vec!["help".into(), " ".into(), "status".into(), "help".into()],
            )
            .unwrap();
        assert_eq!(kept, ["help", "status"]);
        assert_eq!(c.get("simon"), ["help", "status"]);
        assert!(c.get("blink").is_empty());
        let many: Vec<String> = (0..30).map(|i| format!("cmd {i}")).collect();
        assert_eq!(c.set("", many).unwrap().len(), MAX_COMMANDS);
        assert!(c.set("simon", vec![]).unwrap().is_empty());
        assert!(c.get("simon").is_empty());
    }
}
