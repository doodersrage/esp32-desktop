//! Boot loops: a board (or simulation) that resets over and over, each boot printing
//! the ROM's `rst:0x… (REASON)` line. Seen once [`RESETS`] resets come within
//! [`WINDOW`], it is said once, with the last reset's reason and the last crash line
//! before it, until the resets stop for [`QUIET`].

use std::collections::VecDeque;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use regex::Regex;
use serde::Serialize;

/// Resets within [`WINDOW`] that make a boot loop.
pub const RESETS: usize = 3;
pub const WINDOW: Duration = Duration::from_secs(30);
/// Without a reset for this long, a later loop is said again.
pub const QUIET: Duration = Duration::from_secs(60);

static RESET: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"rst:0x[0-9a-fA-F]+ \(([A-Z0-9_]+)\)").expect("valid regex"));
static ESCAPES: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\x1b\[[0-9;]*[A-Za-z]").expect("valid regex"));

/// What a boot loop looked like when it was seen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BootLoop {
    /// Resets seen within the window.
    pub resets: usize,
    /// Over how many seconds.
    pub seconds: u64,
    /// The last reset's reason, as the ROM names it (`TG1WDT_SYS_RESET`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The last crash line before it (`Guru Meditation Error: …`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub crash: Option<String>,
}

#[derive(Default)]
pub struct Watcher {
    line: Vec<u8>,
    resets: VecDeque<Instant>,
    reason: Option<String>,
    crash: Option<String>,
    said: bool,
    asked_until: Option<Instant>,
}

/// Lines longer than this are cut: a reset line is short.
const MAX_LINE: usize = 512;

impl Watcher {
    pub fn new() -> Self {
        Self::default()
    }

    /// The user reset it (sim.reset, serial.reset): the next reset within five seconds
    /// is theirs, not the firmware's.
    pub fn asked(&mut self, now: Instant) {
        self.asked_until = Some(now + Duration::from_secs(5));
    }

    /// Output as it comes, at `now`; a boot loop the first time one is seen.
    pub fn feed(&mut self, bytes: &[u8], now: Instant) -> Option<BootLoop> {
        let mut seen = None;
        for &b in bytes {
            if b != b'\n' {
                if self.line.len() < MAX_LINE {
                    self.line.push(b);
                }
                continue;
            }
            let text = String::from_utf8_lossy(&self.line).into_owned();
            self.line.clear();
            let text = ESCAPES.replace_all(&text, "");
            let text = text.trim();
            if crate::style::CRASHES.iter().any(|c| text.contains(c)) {
                self.crash = Some(text.to_owned());
                continue;
            }
            let Some(c) = RESET.captures(text) else {
                continue;
            };
            // A power-on is a start, not a loop (the reset button, plugging in).
            if &c[1] == "POWERON_RESET" {
                continue;
            }
            if self.asked_until.take().is_some_and(|until| now <= until) {
                continue;
            }
            if self
                .resets
                .back()
                .is_some_and(|&last| now.duration_since(last) >= QUIET)
            {
                self.resets.clear();
                self.said = false;
            }
            self.resets.push_back(now);
            while self
                .resets
                .front()
                .is_some_and(|&t| now.duration_since(t) > WINDOW)
            {
                self.resets.pop_front();
            }
            self.reason = Some(c[1].to_owned());
            if !self.said && self.resets.len() >= RESETS {
                self.said = true;
                let first = *self.resets.front().unwrap_or(&now);
                seen = Some(BootLoop {
                    resets: self.resets.len(),
                    seconds: now.duration_since(first).as_secs().max(1),
                    reason: self.reason.clone(),
                    crash: self.crash.clone(),
                });
            }
        }
        seen
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOOT: &[u8] = b"Guru Meditation Error: Core  1 panic'ed (LoadProhibited)\r\nRebooting...\r\nets Jul 29 2019 12:21:46\r\n\r\nrst:0xc (SW_CPU_RESET),boot:0x13 (SPI_FAST_FLASH_BOOT)\r\n";

    #[test]
    fn says_a_boot_loop_once() {
        let t0 = Instant::now();
        let mut w = Watcher::new();
        assert_eq!(w.feed(b"rst:0x1 (POWERON_RESET),boot:0x13\n", t0), None);
        assert_eq!(w.feed(BOOT, t0 + Duration::from_secs(2)), None);
        assert_eq!(w.feed(BOOT, t0 + Duration::from_secs(4)), None);
        let seen = w
            .feed(BOOT, t0 + Duration::from_secs(6))
            .expect("a boot loop");
        assert_eq!(seen.resets, 3);
        assert_eq!(seen.seconds, 4);
        assert_eq!(seen.reason.as_deref(), Some("SW_CPU_RESET"));
        assert_eq!(
            seen.crash.as_deref(),
            Some("Guru Meditation Error: Core  1 panic'ed (LoadProhibited)")
        );
        // Once, while it goes on.
        assert_eq!(w.feed(BOOT, t0 + Duration::from_secs(8)), None);
        // Quiet for a minute, then looping again: said again.
        let later = t0 + Duration::from_secs(80);
        assert_eq!(w.feed(BOOT, later), None);
        assert_eq!(w.feed(BOOT, later + Duration::from_secs(1)), None);
        assert!(w.feed(BOOT, later + Duration::from_secs(2)).is_some());
    }

    #[test]
    fn resets_the_user_asked_for_do_not_count() {
        let t0 = Instant::now();
        let mut w = Watcher::new();
        for i in 0..4 {
            let at = t0 + Duration::from_secs(i);
            w.asked(at);
            assert_eq!(w.feed(BOOT, at), None);
        }
    }

    #[test]
    fn slow_resets_are_not_a_loop() {
        let t0 = Instant::now();
        let mut w = Watcher::new();
        for i in 0..5 {
            assert_eq!(w.feed(BOOT, t0 + Duration::from_secs(20 * i)), None);
        }
    }

    #[test]
    fn reads_reset_lines_across_chunks_and_colours() {
        let t0 = Instant::now();
        let mut w = Watcher::new();
        let mut seen = None;
        for i in 0..3 {
            let at = t0 + Duration::from_secs(i);
            assert!(w.feed(b"\x1b[0;31mrst:0x8 (TG1WDT", at).is_none());
            seen = w.feed(b"_SYS_RESET),boot:0x13\n", at);
        }
        assert_eq!(seen.unwrap().reason.as_deref(), Some("TG1WDT_SYS_RESET"));
    }
}
