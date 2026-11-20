//! Whether a run prints what its project expects: the manifest's `expect` (the site's
//! lab expectations), patterns that should all appear within some seconds. The daemon
//! feeds a simulation's output through a [`Checker`] and reports its progress.

use regex::Regex;
use serde::{Deserialize, Serialize};

/// A project's expectation, as the manifest gives it.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct Expect {
    /// Regular expressions (the site writes them for JavaScript; the simple ones it
    /// uses read the same here), each matched anywhere in the output, line by line.
    pub patterns: Vec<String>,
    /// How long a board takes to print them all.
    pub seconds: u32,
    /// What the output needs beyond the board: none, wifi, or the project's parts.
    pub needs: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

impl Expect {
    /// Read the manifest's `expect` value; None if it is absent or not one.
    pub fn from_value(v: &serde_json::Value) -> Option<Expect> {
        serde_json::from_value(v.clone()).ok()
    }
}

/// How far a run has got: which patterns have appeared.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Progress {
    pub matched: Vec<String>,
    pub missing: Vec<String>,
}

impl Progress {
    pub fn complete(&self) -> bool {
        self.missing.is_empty()
    }
}

/// Watches output for an expectation's patterns. Patterns that are not valid here are
/// left out (never reported as missing), so a site pattern this program cannot read
/// does not fail a run.
pub struct Checker {
    patterns: Vec<(String, Regex, bool)>,
    /// The line being received, until its newline.
    line: String,
}

/// The longest line kept while waiting for its end; output with no newlines is
/// checked in pieces this long.
const MAX_LINE: usize = 4096;

impl Checker {
    pub fn new(expect: &Expect) -> Checker {
        Checker {
            patterns: expect
                .patterns
                .iter()
                .filter_map(|p| Regex::new(p).ok().map(|r| (p.clone(), r, false)))
                .collect(),
            line: String::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }

    pub fn progress(&self) -> Progress {
        let (matched, missing): (Vec<_>, Vec<_>) = self.patterns.iter().partition(|(_, _, m)| *m);
        Progress {
            matched: matched.into_iter().map(|(p, _, _)| p.clone()).collect(),
            missing: missing.into_iter().map(|(p, _, _)| p.clone()).collect(),
        }
    }

    /// Feed output bytes; Some(progress) when a pattern was seen for the first time.
    pub fn feed(&mut self, bytes: &[u8]) -> Option<Progress> {
        let mut changed = false;
        for ch in String::from_utf8_lossy(bytes).chars() {
            if ch == '\n' || self.line.len() >= MAX_LINE {
                changed |= self.check_line();
                self.line.clear();
                if ch == '\n' {
                    continue;
                }
            }
            if ch != '\r' {
                self.line.push(ch);
            }
        }
        // A prompt with no newline yet ("Touch a key") counts as soon as it is there.
        changed |= self.check_line();
        changed.then(|| self.progress())
    }

    fn check_line(&mut self) -> bool {
        let mut changed = false;
        for (_, re, matched) in &mut self.patterns {
            if !*matched && re.is_match(&self.line) {
                *matched = true;
                changed = true;
            }
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expect(patterns: &[&str]) -> Expect {
        Expect {
            patterns: patterns.iter().map(|s| s.to_string()).collect(),
            seconds: 8,
            needs: "none".into(),
            notes: None,
        }
    }

    #[test]
    fn reports_each_pattern_once_as_it_appears() {
        // Patterns from esp32bench.dev's deep-sleep project.
        let mut c = Checker::new(&expect(&[
            r"Boot \d+, wake cause \d+",
            r"Sleeping\. Press the button to wake\.",
        ]));
        assert_eq!(c.progress().matched.len(), 0);
        assert_eq!(c.feed(b"ets Jul 29 2019\r\nBoot 1, wake"), None);
        let p = c.feed(b" cause 0\r\n").unwrap();
        assert_eq!(p.matched, [r"Boot \d+, wake cause \d+"]);
        assert!(!p.complete());
        assert_eq!(c.feed(b"Boot 2, wake cause 4\n"), None); // seen already
        let p = c.feed(b"Sleeping. Press the button to wake.\n").unwrap();
        assert!(p.complete());
    }

    #[test]
    fn a_prompt_without_a_newline_counts() {
        let mut c = Checker::new(&expect(&["Touch a key"]));
        assert!(c.feed(b"Touch a key").unwrap().complete());
    }

    #[test]
    fn unreadable_patterns_are_left_out() {
        let c = Checker::new(&expect(&["(unclosed", r"tick \d+ ms"]));
        assert_eq!(c.progress().missing, [r"tick \d+ ms"]);
        assert!(Checker::new(&expect(&["(unclosed"])).is_empty());
    }

    #[test]
    fn reads_the_manifests_expect() {
        let v = serde_json::json!({ "patterns": ["tick \\d+ ms"], "seconds": 8, "needs": "none" });
        let e = Expect::from_value(&v).unwrap();
        assert_eq!(e.seconds, 8);
        assert!(Expect::from_value(&serde_json::json!("nope")).is_none());
    }
}
