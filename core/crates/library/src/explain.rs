//! Plain-language explanations of output lines (a crash, a reset reason, an upload
//! error), from the rules the site publishes in the manifest (`explain`): the same
//! rules as esp32bench.dev's bench and paste box. The daemon feeds simulator, serial
//! and build output through an [`Explainer`], which reports each finding once.

use std::collections::HashSet;

use regex::Regex;
use serde::{Deserialize, Serialize};

/// One rule, as the manifest gives it.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct Rule {
    /// The finding's id: one explanation per key per run.
    pub key: String,
    /// A regular expression (JavaScript's syntax in the subset this reads too).
    pub pattern: String,
    /// "i" for case-insensitive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flags: Option<String>,
    pub text: String,
    /// "problem" (show it), or "info" (a note).
    pub level: String,
    /// The site's page about it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// A line's explanation, as events carry it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Finding {
    pub key: String,
    pub text: String,
    pub level: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

pub struct Explainer {
    rules: Vec<(Regex, Rule)>,
    seen: HashSet<String>,
    line: String,
}

/// Lines longer than this are checked in pieces.
const MAX_LINE: usize = 4096;

impl Explainer {
    /// Rules whose pattern this program cannot read are left out.
    pub fn new(rules: &[Rule]) -> Explainer {
        Explainer {
            rules: rules
                .iter()
                .filter_map(|r| {
                    let source = if r.flags.as_deref() == Some("i") {
                        format!("(?i){}", r.pattern)
                    } else {
                        r.pattern.clone()
                    };
                    Regex::new(&source).ok().map(|re| (re, r.clone()))
                })
                .collect(),
            seen: HashSet::new(),
            line: String::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// Feed output bytes; findings in the lines they complete, each key once.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Finding> {
        let mut found = Vec::new();
        for ch in String::from_utf8_lossy(bytes).chars() {
            if ch == '\n' || self.line.len() >= MAX_LINE {
                let line = std::mem::take(&mut self.line);
                found.extend(self.line_findings(&line));
                if ch == '\n' {
                    continue;
                }
            }
            if ch != '\r' {
                self.line.push(ch);
            }
        }
        found
    }

    /// Findings in one whole line (a build log's, which comes a line at a time).
    pub fn line_findings(&mut self, line: &str) -> Vec<Finding> {
        let mut found = Vec::new();
        for (re, rule) in &self.rules {
            if !self.seen.contains(&rule.key) && re.is_match(line) {
                self.seen.insert(rule.key.clone());
                found.push(Finding {
                    key: rule.key.clone(),
                    text: rule.text.clone(),
                    level: rule.level.clone(),
                    url: rule.url.clone(),
                });
            }
        }
        found
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(key: &str, pattern: &str, flags: Option<&str>) -> Rule {
        Rule {
            key: key.into(),
            pattern: pattern.into(),
            flags: flags.map(Into::into),
            text: format!("{key} explained"),
            level: "problem".into(),
            url: Some(format!("https://esp32bench.dev/errors/{key}/")),
        }
    }

    #[test]
    fn explains_each_finding_once() {
        // Patterns as the site generates them.
        let mut e = Explainer::new(&[
            rule(
                "panic-load",
                r"Guru Meditation Error: Core\s+\d panic'ed \((?:LoadProhibited|Load access fault)",
                None,
            ),
            rule("download", "waiting for download", Some("i")),
        ]);
        assert!(
            e.feed(b"ets Jul 29 2019\r\nGuru Meditation Error: Core  1 panic'ed (Load")
                .is_empty()
        );
        let f = e.feed(b"Prohibited). Exception was unhandled.\r\n");
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].key, "panic-load");
        assert_eq!(
            f[0].url.as_deref(),
            Some("https://esp32bench.dev/errors/panic-load/")
        );
        // Once per run.
        assert!(
            e.feed(b"Guru Meditation Error: Core  0 panic'ed (LoadProhibited).\n")
                .is_empty()
        );
        assert_eq!(e.line_findings("Waiting For Download")[0].key, "download");
    }

    #[test]
    fn rules_it_cannot_read_are_left_out() {
        let e = Explainer::new(&[rule("bad", "(?<=x)y", None), rule("ok", "ok", None)]);
        assert!(!e.is_empty());
        assert_eq!(e.rules.len(), 1);
    }
}
