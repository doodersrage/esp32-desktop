//! Console output as people should read it: the terminal colour codes ESP-IDF's log
//! puts around each line removed, and each finished line's level (error, warning,
//! ...) and tag (`wifi`, a source file), so the app can colour and filter lines.
//! The raw bytes are still sent as they came.
//!
//! Levels come from the log formats: ESP-IDF's `E (312) wifi: ...`, Arduino-ESP32's
//! `[   312][E][WiFiGeneric.cpp:1061] ...`, crash and reset lines, and failing that
//! the line's colour (red, yellow, green).

use regex::Regex;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Error,
    Warning,
    Info,
    Debug,
    Verbose,
}

/// What a finished line is; both empty for plain output.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct LineInfo {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<Level>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
}

/// The text to show for a chunk, and one LineInfo per line the chunk finished.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Styled {
    pub text: String,
    pub lines: Vec<LineInfo>,
}

enum Escape {
    None,
    Started,
    Csi(String),
}

/// Reads a stream's bytes in chunks; escape sequences and UTF-8 characters may be
/// split between them.
pub struct Styler {
    pending: Vec<u8>,
    escape: Escape,
    colour: Option<Level>,
    line: String,
    line_colour: Option<Level>,
    idf: Regex,
    arduino: Regex,
}

/// Longest line kept for working out its level; the rest is shown, not read.
const MAX_LINE: usize = 512;

impl Default for Styler {
    fn default() -> Self {
        Styler::new()
    }
}

impl Styler {
    pub fn new() -> Self {
        Styler {
            pending: Vec::new(),
            escape: Escape::None,
            colour: None,
            line: String::new(),
            line_colour: None,
            // Proven at compile time: fixed patterns.
            idf: Regex::new(r"^([EWIDV]) \(\d+\) ([^:\s][^:]{0,39}):").unwrap(),
            arduino: Regex::new(r"^\[\s*\d+\]\[([EWIDV])\]\[([^\]:]{1,60})").unwrap(),
        }
    }

    pub fn feed(&mut self, bytes: &[u8]) -> Styled {
        self.pending.extend_from_slice(bytes);
        let mut out = Styled::default();
        let mut text = String::new();
        loop {
            match std::str::from_utf8(&self.pending) {
                Ok(s) => {
                    text.push_str(s);
                    self.pending.clear();
                    break;
                }
                Err(e) => {
                    let good = e.valid_up_to();
                    // from_utf8 said these bytes are valid.
                    text.push_str(std::str::from_utf8(&self.pending[..good]).unwrap_or_default());
                    match e.error_len() {
                        // A character split by the chunk: wait for the rest.
                        None => {
                            self.pending.drain(..good);
                            break;
                        }
                        Some(n) => {
                            text.push('\u{fffd}');
                            self.pending.drain(..good + n);
                        }
                    }
                }
            }
        }
        for ch in text.chars() {
            self.char(ch, &mut out);
        }
        out
    }

    fn char(&mut self, ch: char, out: &mut Styled) {
        match &mut self.escape {
            Escape::Started => {
                self.escape = if ch == '[' {
                    Escape::Csi(String::new())
                } else {
                    Escape::None
                };
                return;
            }
            Escape::Csi(params) => {
                if ('\u{40}'..='\u{7e}').contains(&ch) {
                    if ch == 'm' {
                        let p = std::mem::take(params);
                        self.colour_from(&p);
                    }
                    self.escape = Escape::None;
                } else if params.len() < 32 {
                    params.push(ch);
                } else {
                    self.escape = Escape::None;
                }
                return;
            }
            Escape::None => {}
        }
        match ch {
            '\u{1b}' => self.escape = Escape::Started,
            '\n' => {
                out.text.push('\n');
                let line = std::mem::take(&mut self.line);
                out.lines.push(self.classify(&line));
                self.line_colour = None;
            }
            '\r' | '\t' => out.text.push(ch),
            c if c.is_control() => {}
            c => {
                if self.line_colour.is_none() {
                    self.line_colour = self.colour;
                }
                if self.line.len() < MAX_LINE {
                    self.line.push(c);
                }
                out.text.push(c);
            }
        }
    }

    fn colour_from(&mut self, params: &str) {
        for p in params.split(';') {
            match p {
                "" | "0" | "39" => self.colour = None,
                "31" | "91" => self.colour = Some(Level::Error),
                "33" | "93" => self.colour = Some(Level::Warning),
                "32" | "92" => self.colour = Some(Level::Info),
                _ => {}
            }
        }
    }

    fn classify(&self, line: &str) -> LineInfo {
        let line = line.trim_end_matches('\r');
        let level = |c: &str| match c {
            "E" => Some(Level::Error),
            "W" => Some(Level::Warning),
            "I" => Some(Level::Info),
            "D" => Some(Level::Debug),
            _ => Some(Level::Verbose),
        };
        if let Some(c) = self
            .idf
            .captures(line)
            .or_else(|| self.arduino.captures(line))
        {
            return LineInfo {
                level: level(&c[1]),
                tag: Some(c[2].trim().to_owned()),
            };
        }
        const CRASHES: &[&str] = &[
            "Guru Meditation Error",
            "abort() was called",
            "Brownout detector was triggered",
            "assert failed",
            "Stack smashing protect failure",
            "CORRUPT HEAP",
        ];
        if CRASHES.iter().any(|c| line.contains(c)) {
            return LineInfo {
                level: Some(Level::Error),
                tag: None,
            };
        }
        LineInfo {
            level: self.line_colour,
            tag: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(level: Level, tag: &str) -> LineInfo {
        LineInfo {
            level: Some(level),
            tag: Some(tag.into()),
        }
    }

    #[test]
    fn removes_colour_codes_and_reads_idf_lines() {
        let mut s = Styler::new();
        let out = s.feed(b"\x1b[0;32mI (312) wifi: connected\x1b[0m\n\x1b[0;31mE (400) app: no sensor\x1b[0m\nplain\n");
        assert_eq!(
            out.text,
            "I (312) wifi: connected\nE (400) app: no sensor\nplain\n"
        );
        assert_eq!(
            out.lines,
            [
                info(Level::Info, "wifi"),
                info(Level::Error, "app"),
                LineInfo::default()
            ]
        );
    }

    #[test]
    fn reads_arduino_and_crash_lines_and_colours() {
        let mut s = Styler::new();
        let out = s.feed(
            b"[   312][W][WiFiGeneric.cpp:1061] _eventCallback(): Reason: 2\nGuru Meditation Error: Core  1 panic'ed\n\x1b[33mcareful\x1b[0m\n",
        );
        assert_eq!(out.lines[0], info(Level::Warning, "WiFiGeneric.cpp"));
        assert_eq!(out.lines[1].level, Some(Level::Error));
        assert_eq!(out.lines[2].level, Some(Level::Warning));
        assert_eq!(out.lines[2].tag, None);
    }

    #[test]
    fn copes_with_pieces() {
        let mut s = Styler::new();
        // An escape, a line and a character, each split between chunks.
        let mut text = String::new();
        let mut lines = Vec::new();
        for piece in [
            &b"\x1b[0;3"[..],
            b"1mE (1) t",
            b"ag: caf\xc3",
            b"\xa9\x1b[0m",
            b"\n",
        ] {
            let out = s.feed(piece);
            text.push_str(&out.text);
            lines.extend(out.lines);
        }
        assert_eq!(text, "E (1) tag: café\n");
        assert_eq!(lines, [info(Level::Error, "tag")]);
        // Bytes that are not UTF-8 show as replacement characters.
        assert_eq!(s.feed(b"\xff\n").text, "\u{fffd}\n");
    }
}
