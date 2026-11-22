//! Watching the user's own project for saves (phase 7.4): snapshots of its files,
//! compared. Polling rather than the OS's notifications: a project is a handful of
//! files, it behaves the same on Linux, macOS and Windows, and it needs nothing more.
//!
//! Left out: build folders, version control, hidden files (`.clangd`, `.vscode`) and
//! the temporary and backup files editors write while saving, so a save is seen once
//! and a build does not trigger itself.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::SystemTime;

/// At most this many files are watched, and folders this deep.
const MAX_FILES: usize = 5000;
const MAX_DEPTH: usize = 8;

/// Folders never watched: build output and downloaded components.
const SKIP_DIRS: &[&str] = &["build", "managed_components", "node_modules", "__pycache__"];

/// What a project's files were at one moment: each file's size and modification time,
/// by its path relative to the project.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Snapshot(BTreeMap<String, (u64, Option<SystemTime>)>);

/// Whether a file's name is one an editor writes while saving, or a backup of it:
/// `.x.swp` (Vim), `x~`, `.#x` and `#x#` (Emacs), `x.kate-swp`, `4913` (Vim's probe).
pub fn is_temporary(name: &str) -> bool {
    name.starts_with('.')
        || name.ends_with('~')
        || (name.starts_with('#') && name.ends_with('#'))
        || name.ends_with(".kate-swp")
        || name.ends_with(".swp")
        || name.ends_with(".swx")
        || name.ends_with(".tmp")
        || name == "4913"
}

impl Snapshot {
    /// The files under `root` now.
    pub fn scan(root: &Path) -> Self {
        let mut files = BTreeMap::new();
        walk(root, root, 0, &mut files);
        Snapshot(files)
    }

    /// The files added, removed or changed since `before`, relative to the project.
    pub fn changed_since(&self, before: &Snapshot) -> Vec<String> {
        let mut out: Vec<String> = self
            .0
            .iter()
            .filter(|(p, v)| before.0.get(*p) != Some(v))
            .map(|(p, _)| p.clone())
            .collect();
        out.extend(
            before
                .0
                .keys()
                .filter(|p| !self.0.contains_key(*p))
                .cloned(),
        );
        out.sort();
        out.dedup();
        out
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// The breakpoints among `kept` (as breakpoints.set keeps them: a function, or
/// `file:line`, maybe with ` if <condition>`) whose line is no longer in its file: the
/// file is in the project at `root` (by its path in it, or by its name) and has fewer
/// lines now. A function's breakpoint, or one in a file outside the project, stays.
pub fn stale_breakpoints(root: &Path, snapshot: &Snapshot, kept: &[String]) -> Vec<String> {
    kept.iter()
        .filter(|k| {
            let location = k.split_once(" if ").map_or(k.as_str(), |(l, _)| l).trim();
            let Some((file, line)) = location.rsplit_once(':') else {
                return false;
            };
            let Ok(line) = line.parse::<usize>() else {
                return false;
            };
            let found = snapshot
                .0
                .keys()
                .find(|rel| rel.as_str() == file || rel.rsplit('/').next() == Some(file));
            let Some(rel) = found else {
                return false;
            };
            let Ok(text) = std::fs::read(root.join(rel)) else {
                return false;
            };
            let lines = text.split(|b| *b == b'\n').count();
            line > lines
        })
        .cloned()
        .collect()
}

fn walk(
    root: &Path,
    dir: &Path,
    depth: usize,
    files: &mut BTreeMap<String, (u64, Option<SystemTime>)>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        if files.len() >= MAX_FILES {
            return;
        }
        let name = e.file_name().to_string_lossy().into_owned();
        if is_temporary(&name) {
            continue;
        }
        let Ok(kind) = e.file_type() else { continue };
        let path = e.path();
        if kind.is_dir() {
            // build/ and anything named like a build folder (build-esp32, idf-build).
            if depth < MAX_DEPTH && !SKIP_DIRS.contains(&name.as_str()) && !name.contains("build") {
                walk(root, &path, depth + 1, files);
            }
        } else if kind.is_file() {
            let Ok(meta) = e.metadata() else { continue };
            let rel = path
                .strip_prefix(root)
                .map(|r| r.to_string_lossy().replace('\\', "/"))
                .unwrap_or(name);
            files.insert(rel, (meta.len(), meta.modified().ok()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sees_saves_and_not_editor_litter_or_builds() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path();
        std::fs::create_dir_all(p.join("main")).unwrap();
        std::fs::create_dir_all(p.join("build/esp32")).unwrap();
        std::fs::write(p.join("main/app.c"), "int a;").unwrap();
        std::fs::write(p.join("CMakeLists.txt"), "x").unwrap();
        let before = Snapshot::scan(p);
        assert_eq!(before.len(), 2);
        // An editor's litter, a build's output, the app's .clangd: not seen.
        for f in [
            ".app.c.swp",
            "app.c~",
            "main/.#app.c",
            "main/app.c.kate-swp",
            "4913",
            ".clangd",
            "build/esp32/x.o",
        ] {
            std::fs::write(p.join(f), "x").unwrap();
        }
        assert!(Snapshot::scan(p).changed_since(&before).is_empty());
        // A save (a different size), a new file and a removed one: seen.
        std::fs::write(p.join("main/app.c"), "int a; int b;").unwrap();
        std::fs::write(p.join("main/util.h"), "").unwrap();
        std::fs::remove_file(p.join("CMakeLists.txt")).unwrap();
        assert_eq!(
            Snapshot::scan(p).changed_since(&before),
            ["CMakeLists.txt", "main/app.c", "main/util.h"]
        );
    }

    #[test]
    fn finds_breakpoints_past_the_end_of_their_file() {
        let t = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(t.path().join("main")).unwrap();
        std::fs::write(t.path().join("main/app.c"), "a\nb\nc\n").unwrap();
        let snap = Snapshot::scan(t.path());
        let kept: Vec<String> = [
            "main/app.c:2",
            "app.c:40",
            "app.c:9 if x > 3",
            "loop",
            "Arduino.h:500",
        ]
        .map(String::from)
        .into();
        assert_eq!(
            stale_breakpoints(t.path(), &snap, &kept),
            ["app.c:40", "app.c:9 if x > 3"]
        );
    }

    #[test]
    fn knows_editor_temporaries() {
        for n in [".x.swp", "x~", "#x#", ".#x", "x.kate-swp", "4913", "x.tmp"] {
            assert!(is_temporary(n), "{n}");
        }
        for n in ["main.c", "Blink.ino", "sdkconfig", "CMakeLists.txt"] {
            assert!(!is_temporary(n), "{n}");
        }
    }
}
