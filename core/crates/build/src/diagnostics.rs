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

/// How much of the board the firmware takes, from arduino-cli's two size lines:
/// `Sketch uses N bytes (P%) of program storage space. Maximum is M bytes.` and
/// `Global variables use N bytes (P%) of dynamic memory, ... Maximum is M bytes.`
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Size {
    pub flash: u64,
    pub flash_max: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ram: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ram_max: Option<u64>,
}

/// Reads log lines for the size lines.
#[derive(Default)]
pub struct SizeReader {
    size: Option<Size>,
}

impl SizeReader {
    pub fn line(&mut self, line: &str) {
        // The first number and the one after "Maximum is".
        let numbers = |l: &str| -> Option<(u64, u64)> {
            let used = l.split_whitespace().find_map(|w| w.parse::<u64>().ok())?;
            let (_, rest) = l.split_once("Maximum is ")?;
            let max = rest.split_whitespace().next()?.parse().ok()?;
            Some((used, max))
        };
        if line.starts_with("Sketch uses ") {
            if let Some((used, max)) = numbers(line) {
                self.size = Some(Size {
                    flash: used,
                    flash_max: max,
                    ..Size::default()
                });
            }
        } else if line.starts_with("Global variables use ")
            && let (Some(size), Some((used, max))) = (self.size.as_mut(), numbers(line))
        {
            size.ram = Some(used);
            size.ram_max = Some(max);
        }
    }

    pub fn size(&self) -> Option<&Size> {
        self.size.as_ref()
    }
}

/// Whether a build log line is a compiler or tool error or warning, for the app to
/// count, colour and filter.
pub fn line_level(line: &str) -> Option<&'static str> {
    if line.contains(": error:")
        || line.contains(": fatal error:")
        || line.contains(": undefined reference to ")
        || line.starts_with("Error ")
        || line.starts_with("Error:")
        || line.contains("collect2: error:")
    {
        Some("error")
    } else if line.contains(": warning:") || line.starts_with("WARNING:") {
        Some("warning")
    } else {
        None
    }
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
    fn reads_the_firmware_size() {
        let mut r = SizeReader::default();
        r.line(
            "Sketch uses 289925 bytes (22%) of program storage space. Maximum is 1310720 bytes.",
        );
        r.line("Global variables use 21056 bytes (6%) of dynamic memory, leaving 306624 bytes for local variables. Maximum is 327680 bytes.");
        assert_eq!(
            r.size(),
            Some(&Size {
                flash: 289925,
                flash_max: 1310720,
                ram: Some(21056),
                ram_max: Some(327680)
            })
        );
        let mut none = SizeReader::default();
        none.line("Sketch uses 1234 bytes");
        assert_eq!(none.size(), None);
    }

    #[test]
    fn tells_errors_and_warnings() {
        assert_eq!(line_level("a.ino:3:1: error: x"), Some("error"));
        assert_eq!(
            line_level("Error during build: exit status 1"),
            Some("error")
        );
        assert_eq!(
            line_level("a.ino:3:1: warning: unused variable 'x'"),
            Some("warning")
        );
        assert_eq!(line_level("Sketch uses 1234 bytes"), None);
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
