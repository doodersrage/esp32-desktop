//! The first compiler or linker error in a build's log, so the app can say what
//! failed instead of only that the build did. GCC's formats:
//! `file:line:column: error: message`, `file:line: fatal error: message` and the
//! linker's `file:line: undefined reference to ...`.

use std::path::{Path, PathBuf};

use regex::Regex;
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FirstError {
    /// The file as people know it: relative to a local project's folder, else its
    /// name (library sketches build in a private folder).
    pub file: String,
    /// The file itself, for opening it: only for a file inside a local project.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    pub line: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column: Option<u32>,
    pub message: String,
}

/// Reads log lines until it has seen the first error.
pub struct Finder {
    compile: Regex,
    link: Regex,
    project: Option<PathBuf>,
    found: Option<FirstError>,
}

impl Finder {
    /// `project`: a local project's folder, whose files may be opened.
    pub fn new(project: Option<&Path>) -> Self {
        Finder {
            // Proven at compile time: fixed patterns.
            compile: Regex::new(r"^(.+?):(\d+):(?:(\d+):)? (?:fatal )?error: (.+)$").unwrap(),
            link: Regex::new(r"^(.+?):(\d+): (undefined reference to .+)$").unwrap(),
            project: project.and_then(|p| p.canonicalize().ok()),
            found: None,
        }
    }

    pub fn line(&mut self, line: &str) {
        if self.found.is_some() {
            return;
        }
        let line = line.trim_end();
        let Some(c) = self
            .compile
            .captures(line)
            .or_else(|| self.link.captures(line))
        else {
            return;
        };
        let raw = Path::new(&c[1]);
        let message = c.get(c.len() - 1).map_or("", |m| m.as_str()).to_owned();
        let column = if c.len() == 5 {
            c.get(3).and_then(|m| m.as_str().parse().ok())
        } else {
            None
        };
        let (file, path) = self.name(raw);
        self.found = Some(FirstError {
            file,
            path,
            line: c[2].parse().unwrap_or(0),
            column,
            message,
        });
    }

    pub fn first(&self) -> Option<&FirstError> {
        self.found.as_ref()
    }

    fn name(&self, raw: &Path) -> (String, Option<PathBuf>) {
        if let Some(root) = &self.project
            && let Ok(real) = raw.canonicalize()
            && let Ok(rel) = real.strip_prefix(root)
        {
            return (rel.to_string_lossy().replace('\\', "/"), Some(real));
        }
        let name = raw
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| raw.to_string_lossy().into_owned());
        (name, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_first_error_only() {
        let mut f = Finder::new(None);
        for l in [
            "Compiling sketch...",
            "/home/u/.cache/esp32bench/builds/blink/esp32/blink/blink.ino: In function 'void loop()':",
            "/home/u/.cache/esp32bench/builds/blink/esp32/blink/blink.ino:12:3: error: 'ledd' was not declared in this scope; did you mean 'led'?",
            "/home/u/x/other.cpp:3:1: error: later",
        ] {
            f.line(l);
        }
        let e = f.first().unwrap();
        assert_eq!(e.file, "blink.ino");
        assert_eq!(e.path, None);
        assert_eq!((e.line, e.column), (12, Some(3)));
        assert!(e.message.starts_with("'ledd' was not declared"));
    }

    #[test]
    fn reads_fatal_and_linker_errors() {
        let mut f = Finder::new(None);
        f.line("src/main.c:1:10: fatal error: foo.h: No such file or directory");
        assert_eq!(
            f.first().unwrap().message,
            "foo.h: No such file or directory"
        );
        let mut f = Finder::new(None);
        f.line("warning: unused variable");
        f.line("/tmp/x/main.cpp:40: undefined reference to `setupWifi()'");
        let e = f.first().unwrap();
        assert_eq!((e.file.as_str(), e.line, e.column), ("main.cpp", 40, None));
        assert!(e.message.starts_with("undefined reference"));
    }

    #[test]
    fn a_local_projects_files_can_be_opened() {
        let t = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(t.path().join("main")).unwrap();
        let file = t.path().join("main/app.c");
        std::fs::write(&file, "x").unwrap();
        let mut f = Finder::new(Some(t.path()));
        f.line(&format!("{}:7:5: error: expected ';'", file.display()));
        let e = f.first().unwrap();
        assert_eq!(e.file, "main/app.c");
        assert_eq!(
            e.path.as_deref(),
            Some(file.canonicalize().unwrap().as_path())
        );
        // A file outside the project is only named.
        let mut f = Finder::new(Some(t.path()));
        f.line("/usr/include/stdio.h:3:1: error: bad");
        assert_eq!(f.first().unwrap().path, None);
    }
}
