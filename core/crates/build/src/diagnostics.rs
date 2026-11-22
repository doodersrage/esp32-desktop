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

/// Where a compiler error or warning points, for the app to show that line: the
/// same naming as [`FirstError`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Location {
    pub file: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    pub line: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column: Option<u32>,
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
    located: Regex,
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
            located: Regex::new(r"^(.+?):(\d+):(?:(\d+):)? (?:(?:fatal )?error|warning|note): ")
                .unwrap(),
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

    /// Where an error or warning line points, if it names a file and line.
    pub fn location(&self, line: &str) -> Option<Location> {
        let line = line.trim_end();
        let (raw, number, column) = if let Some(c) = self.located.captures(line) {
            (
                c.get(1)?.as_str(),
                c.get(2)?.as_str(),
                c.get(3).and_then(|m| m.as_str().parse().ok()),
            )
        } else {
            let c = self.link.captures(line)?;
            (c.get(1)?.as_str(), c.get(2)?.as_str(), None)
        };
        let line = number.parse().ok().filter(|n| *n > 0)?;
        let (file, path) = self.name(Path::new(raw));
        Some(Location {
            file,
            path,
            line,
            column,
        })
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

/// One compiler or linker message, for a Problems list: where it points (the same
/// naming as [`FirstError`]; a linker message may name no file), how bad it is, what
/// it says, the headers it was reached through, and the compiler's notes on it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostic {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column: Option<u32>,
    /// `error` or `warning`.
    pub severity: &'static str,
    pub message: String,
    /// "In file included from": the includes that led to the file, outermost last.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub included_from: Vec<Location>,
    /// The compiler's notes after it ("declared here", "did you mean").
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<Note>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Note {
    #[serde(flatten)]
    pub at: Location,
    pub message: String,
}

/// At most this many diagnostics are kept from one build.
pub const MAX_DIAGNOSTICS: usize = 500;

/// Reads a build's log (GCC through arduino-cli or idf.py, and the linker) into
/// [`Diagnostic`]s, in the order they came, without repeats.
pub struct Collector {
    names: Finder,
    message: Regex,
    included: Regex,
    included_more: Regex,
    linker: Regex,
    chain: Vec<Location>,
    found: Vec<Diagnostic>,
}

impl Collector {
    /// `project`: a local project's folder, whose files may be opened.
    pub fn new(project: Option<&Path>) -> Self {
        Collector {
            names: Finder::new(project),
            // Proven at compile time: fixed patterns.
            message: Regex::new(r"^(.+?):(\d+):(?:(\d+):)? (fatal error|error|warning|note): (.+)$").unwrap(),
            included: Regex::new(r"^In file included from (.+?):(\d+)[,:]$").unwrap(),
            included_more: Regex::new(r"^\s+from (.+?):(\d+)[,:]$").unwrap(),
            // `file:line: undefined reference to ...`, or with no line (an object file,
            // `(.text+0x12)`), and `multiple definition of`.
            linker: Regex::new(
                r"^(?:(.+?):(?:(\d+)|\([^)]*\)): )?((?:undefined reference to|multiple definition of) .+)$",
            )
            .unwrap(),
            chain: Vec::new(),
            found: Vec::new(),
        }
    }

    pub fn line(&mut self, line: &str) {
        let line = line.trim_end();
        if let Some(c) = self.included.captures(line) {
            self.chain = vec![self.at(&c[1], &c[2], None)];
            return;
        }
        if let Some(c) = self.included_more.captures(line) {
            let at = self.at(&c[1], &c[2], None);
            self.chain.push(at);
            return;
        }
        if let Some(c) = self.message.captures(line) {
            let at = self.at(&c[1], &c[2], c.get(3).map(|m| m.as_str()));
            let message = c[5].to_owned();
            if &c[4] == "note" {
                if let Some(last) = self.found.last_mut() {
                    last.notes.push(Note { at, message });
                }
                return;
            }
            let severity = if &c[4] == "warning" {
                "warning"
            } else {
                "error"
            };
            let chain = std::mem::take(&mut self.chain);
            self.push(Diagnostic {
                file: Some(at.file),
                path: at.path,
                line: Some(at.line),
                column: at.column,
                severity,
                message,
                included_from: chain,
                notes: Vec::new(),
            });
            return;
        }
        if let Some(c) = self.linker.captures(line) {
            let at = match (c.get(1), c.get(2)) {
                (Some(f), Some(n)) => Some(self.at(f.as_str(), n.as_str(), None)),
                _ => None,
            };
            let file = match (&at, c.get(1)) {
                (Some(at), _) => Some(at.file.clone()),
                (None, Some(f)) => Some(self.names.name(Path::new(f.as_str())).0),
                (None, None) => None,
            };
            self.push(Diagnostic {
                file,
                path: at.as_ref().and_then(|a| a.path.clone()),
                line: at.as_ref().map(|a| a.line),
                column: None,
                severity: "error",
                message: c[3].to_owned(),
                included_from: Vec::new(),
                notes: Vec::new(),
            });
            return;
        }
        // Anything else ends an include chain that led nowhere.
        self.chain.clear();
    }

    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.found
    }

    fn at(&self, file: &str, line: &str, column: Option<&str>) -> Location {
        let (file, path) = self.names.name(Path::new(file));
        Location {
            file,
            path,
            line: line.parse().unwrap_or(0),
            column: column.and_then(|c| c.parse().ok()),
        }
    }

    fn push(&mut self, d: Diagnostic) {
        let same = |o: &Diagnostic| {
            o.file == d.file && o.line == d.line && o.column == d.column && o.message == d.message
        };
        if self.found.len() < MAX_DIAGNOSTICS && !self.found.iter().any(same) {
            self.found.push(d);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collects_every_diagnostic() {
        let mut c = Collector::new(None);
        for l in [
            "/b/sketch/blink.ino: In function 'void loop()':",
            "/b/sketch/blink.ino:12:3: error: 'ledd' was not declared in this scope; did you mean 'led'?",
            "/b/sketch/blink.ino:3:11: note: 'led' declared here",
            "/b/sketch/blink.ino:20:7: warning: unused variable 'n' [-Wunused-variable]",
            // The same warning again (a second compile pass): kept once.
            "/b/sketch/blink.ino:20:7: warning: unused variable 'n' [-Wunused-variable]",
            "In file included from /b/sketch/blink.ino:4,",
            "                 from /b/sketch/main.cpp:1:",
            "/b/libs/Thing/Thing.h:7:10: fatal error: Wire2.h: No such file or directory",
            "compilation terminated.",
            "/tools/xtensa-esp32-elf/bin/../lib/gcc/xtensa-esp-elf/ld: /b/sketch/blink.ino.cpp.o:(.literal._Z5setupv+0x0): undefined reference to `startWifi()'",
            "/b/sketch/net.cpp:40: undefined reference to `connect()'",
            "collect2: error: ld returned 1 exit status",
        ] {
            c.line(l);
        }
        let d = c.diagnostics();
        assert_eq!(d.len(), 5, "{d:#?}");
        assert_eq!(
            (d[0].file.as_deref(), d[0].line, d[0].column),
            (Some("blink.ino"), Some(12), Some(3))
        );
        assert_eq!(d[0].severity, "error");
        assert_eq!(d[0].notes.len(), 1);
        assert_eq!(d[0].notes[0].at.line, 3);
        assert_eq!(d[1].severity, "warning");
        // The include chain, innermost first.
        assert_eq!(d[2].file.as_deref(), Some("Thing.h"));
        let chain: Vec<(&str, u32)> = d[2]
            .included_from
            .iter()
            .map(|l| (l.file.as_str(), l.line))
            .collect();
        assert_eq!(chain, [("blink.ino", 4), ("main.cpp", 1)]);
        // A linker error with no line names the object file; one with a line, its source.
        assert!(
            d[3].message
                .starts_with("undefined reference to `startWifi()'"),
            "{:?}",
            d[3]
        );
        assert_eq!(d[3].line, None);
        assert_eq!(
            (d[4].file.as_deref(), d[4].line),
            (Some("net.cpp"), Some(40))
        );
    }

    #[test]
    fn names_a_local_projects_files_by_their_place_in_it() {
        let t = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(t.path().join("main")).unwrap();
        std::fs::write(t.path().join("main/app.c"), "").unwrap();
        let mut c = Collector::new(Some(t.path()));
        c.line(&format!(
            "{}/main/app.c:9:5: error: expected ';'",
            t.path().display()
        ));
        let d = &c.diagnostics()[0];
        assert_eq!(d.file.as_deref(), Some("main/app.c"));
        assert_eq!(
            d.path.as_deref(),
            Some(
                t.path()
                    .join("main/app.c")
                    .canonicalize()
                    .unwrap()
                    .as_path()
            )
        );
    }

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
    fn errors_and_warnings_say_where_they_point() {
        let f = Finder::new(None);
        let l = f
            .location("/tmp/x/sketch/blink.ino:12:3: warning: unused variable 'n'")
            .unwrap();
        assert_eq!(
            (l.file.as_str(), l.line, l.column, l.path),
            ("blink.ino", 12, Some(3), None)
        );
        let l = f
            .location("/tmp/x/main.cpp:40: undefined reference to `setup'")
            .unwrap();
        assert_eq!((l.file.as_str(), l.line, l.column), ("main.cpp", 40, None));
        assert_eq!(
            f.location("Sketch uses 254756 bytes (19%) of program storage space."),
            None
        );
        assert_eq!(
            f.location("collect2: error: ld returned 1 exit status"),
            None
        );
        assert_eq!(f.location("x.c:0: error: nowhere"), None);
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
