//! Signal traces: the edges of a simulation's bridged pins with their simulated
//! times, kept in a bounded buffer, and written out as a VCD (IEEE 1364 value change
//! dump) that PulseView and GTKWave open.
//!
//! Times come from QEMU's virtual clock (`esp32bench-qemu-host`): the guest's own
//! time, which with `-icount` follows the instructions run, so it does not depend on
//! this computer's load the way wall-clock time would.

use std::collections::{BTreeSet, VecDeque};
use std::fmt::Write as _;

use crate::gpio::PinEvent;

/// One change of a pin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Edge {
    /// Simulated time, in nanoseconds since the machine started.
    pub time_ns: u64,
    pub gpio: u8,
    pub level: bool,
    /// Driven by the host as an input (a part or `sim.setPin`), not by the firmware.
    pub input: bool,
}

/// How fast a pin toggles: rising edges per second, and the share of the time it is
/// high (PWM's duty), over whole periods.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
pub struct Rate {
    pub hz: f64,
    pub high: f64,
}

impl Rate {
    /// Whether `other` would read differently: 0.5% in frequency or half a percent
    /// of duty.
    pub fn differs(&self, other: &Rate) -> bool {
        (self.hz - other.hz).abs() > self.hz.abs() * 0.005 || (self.high - other.high).abs() > 0.005
    }
}

/// The latest edges of a session, oldest dropped first once `cap` is reached.
#[derive(Debug, Clone)]
pub struct Trace {
    edges: VecDeque<Edge>,
    cap: usize,
    dropped: u64,
    last_time: u64,
}

impl Default for Trace {
    fn default() -> Self {
        Trace::new(Trace::DEFAULT_CAP)
    }
}

impl Trace {
    /// About 16 MB of edges.
    pub const DEFAULT_CAP: usize = 1_000_000;

    pub fn new(cap: usize) -> Self {
        Trace {
            edges: VecDeque::new(),
            cap: cap.max(1),
            dropped: 0,
            last_time: 0,
        }
    }

    /// Record a pin event if it is a level change, and return the edge. An event
    /// without a time (or with one earlier than the last, which QEMU's clock should
    /// not give) is placed at the last edge's time, so times never go backwards.
    pub fn record(&mut self, ev: &PinEvent) -> Option<Edge> {
        let (gpio, level, time_ns, input) = match *ev {
            PinEvent::Level {
                gpio,
                level,
                time_ns,
            } => (gpio, level, time_ns, false),
            PinEvent::Input {
                gpio,
                level,
                time_ns,
            } => (gpio, level, time_ns, true),
            _ => return None,
        };
        let time_ns = time_ns.unwrap_or(self.last_time).max(self.last_time);
        self.last_time = time_ns;
        let edge = Edge {
            time_ns,
            gpio,
            level,
            input,
        };
        if self.edges.len() == self.cap {
            self.edges.pop_front();
            self.dropped += 1;
        }
        self.edges.push_back(edge);
        Some(edge)
    }

    /// `gpio`'s rate over the `window_ns` before `now_ns`: from its first rising edge
    /// there to its last, so at least two are needed; None for a pin that does not
    /// toggle that often.
    pub fn rate(&self, gpio: u8, now_ns: u64, window_ns: u64) -> Option<Rate> {
        let since = now_ns.saturating_sub(window_ns);
        let mut edges: Vec<&Edge> = self
            .edges
            .iter()
            .rev()
            .take_while(|e| e.time_ns >= since)
            .filter(|e| e.gpio == gpio)
            .collect();
        edges.reverse();
        // Rises: a high after a low (the host may repeat a level).
        let rises: Vec<usize> = (1..edges.len())
            .filter(|&i| edges[i].level && !edges[i - 1].level)
            .collect();
        let (&first, &last) = (rises.first()?, rises.last()?);
        let (start, end) = (edges[first].time_ns, edges[last].time_ns);
        if rises.len() < 2 || end <= start {
            return None;
        }
        let mut high_ns = 0;
        for w in edges[first..=last].windows(2) {
            if w[0].level {
                high_ns += w[1].time_ns - w[0].time_ns;
            }
        }
        let span = (end - start) as f64;
        Some(Rate {
            hz: (rises.len() - 1) as f64 * 1e9 / span,
            high: high_ns as f64 / span,
        })
    }

    pub fn edges(&self) -> impl Iterator<Item = &Edge> {
        self.edges.iter()
    }

    pub fn len(&self) -> usize {
        self.edges.len()
    }

    pub fn is_empty(&self) -> bool {
        self.edges.is_empty()
    }

    /// Edges dropped from the start because the buffer was full.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// The trace as VCD text, one 1-bit wire per GPIO that changed (`GPIO2`, ...),
    /// at 1 ns resolution. Pins are unknown (`x`) until their first recorded edge.
    pub fn to_vcd(&self, version: &str) -> String {
        let gpios: BTreeSet<u8> = self.edges.iter().map(|e| e.gpio).collect();
        // VCD identifiers: printable ASCII from '!'; there are at most 49 GPIOs.
        let id = |gpio: u8| -> char {
            let i = gpios.iter().position(|g| *g == gpio).unwrap_or(0);
            char::from(b'!' + u8::try_from(i).unwrap_or(0))
        };
        let mut out = String::new();
        let _ = writeln!(out, "$version esp32bench {version} $end");
        if self.dropped > 0 {
            let _ = writeln!(
                out,
                "$comment the first {} edges were dropped: the trace keeps the latest {} $end",
                self.dropped, self.cap
            );
        }
        out.push_str("$timescale 1 ns $end\n$scope module esp32 $end\n");
        for g in &gpios {
            let _ = writeln!(out, "$var wire 1 {} GPIO{g} $end", id(*g));
        }
        out.push_str("$upscope $end\n$enddefinitions $end\n#0\n$dumpvars\n");
        for g in &gpios {
            let _ = writeln!(out, "x{}", id(*g));
        }
        out.push_str("$end\n");
        let mut at = None;
        for e in &self.edges {
            if at != Some(e.time_ns) {
                // #0 is already open, from $dumpvars.
                if !(at.is_none() && e.time_ns == 0) {
                    let _ = writeln!(out, "#{}", e.time_ns);
                }
                at = Some(e.time_ns);
            }
            let _ = writeln!(out, "{}{}", u8::from(e.level), id(e.gpio));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    fn level(gpio: u8, level: bool, t: u64) -> PinEvent {
        PinEvent::Level {
            gpio,
            level,
            time_ns: Some(t),
        }
    }

    #[test]
    fn measures_how_fast_a_pin_toggles() {
        let mut t = Trace::default();
        // GPIO 2 at 1 kHz, high 25% of the time; GPIO 4 changes once.
        for n in 0..10u64 {
            t.record(&level(2, true, n * 1_000_000));
            t.record(&level(2, true, n * 1_000_000 + 100)); // a repeated level
            t.record(&level(2, false, n * 1_000_000 + 250_000));
        }
        t.record(&level(4, true, 5_000_000));
        let r = t.rate(2, 10_000_000, 2_000_000_000).unwrap();
        assert!((r.hz - 1000.0).abs() < 1e-6, "{r:?}");
        assert!((r.high - 0.25).abs() < 1e-9, "{r:?}");
        assert_eq!(t.rate(4, 10_000_000, 2_000_000_000), None);
        assert_eq!(t.rate(9, 10_000_000, 2_000_000_000), None);
        // Outside the window: no longer toggling.
        assert_eq!(t.rate(2, 10_000_000_000, 2_000_000_000), None);
        assert!(r.differs(&Rate {
            hz: 1010.0,
            high: 0.25
        }));
        assert!(!r.differs(&Rate {
            hz: 1001.0,
            high: 0.252
        }));
    }

    use super::*;

    fn out(gpio: u8, level: bool, t: u64) -> PinEvent {
        PinEvent::Level {
            gpio,
            level,
            time_ns: Some(t),
        }
    }

    #[test]
    fn records_level_changes_only() {
        let mut t = Trace::default();
        assert!(
            t.record(&PinEvent::Direction {
                gpio: 2,
                output: true
            })
            .is_none()
        );
        let e = t.record(&out(2, true, 1000)).unwrap();
        assert_eq!(
            e,
            Edge {
                time_ns: 1000,
                gpio: 2,
                level: true,
                input: false
            }
        );
        let e = t
            .record(&PinEvent::Input {
                gpio: 4,
                level: false,
                time_ns: None,
            })
            .unwrap();
        assert_eq!((e.time_ns, e.input), (1000, true), "no time: the last one");
        assert_eq!(
            t.record(&out(2, false, 500)).unwrap().time_ns,
            1000,
            "never backwards"
        );
        assert_eq!(t.len(), 3);
    }

    #[test]
    fn keeps_the_latest_when_full() {
        let mut t = Trace::new(2);
        for i in 0..5 {
            t.record(&out(2, i % 2 == 0, i * 10));
        }
        assert_eq!(t.len(), 2);
        assert_eq!(t.dropped(), 3);
        let times: Vec<u64> = t.edges().map(|e| e.time_ns).collect();
        assert_eq!(times, vec![30, 40]);
        assert!(t.to_vcd("0").contains("the first 3 edges were dropped"));
    }

    #[test]
    fn writes_vcd() {
        let mut t = Trace::default();
        t.record(&out(4, true, 0));
        t.record(&out(2, true, 1500));
        t.record(&out(4, false, 1500));
        t.record(&out(2, false, 3000));
        let vcd = t.to_vcd("0.1.5");
        let expected = "\
$version esp32bench 0.1.5 $end
$timescale 1 ns $end
$scope module esp32 $end
$var wire 1 ! GPIO2 $end
$var wire 1 \" GPIO4 $end
$upscope $end
$enddefinitions $end
#0
$dumpvars
x!
x\"
$end
1\"
#1500
1!
0\"
#3000
0!
";
        assert_eq!(vcd, expected);
    }

    #[test]
    fn empty_trace_is_still_valid_vcd() {
        let vcd = Trace::default().to_vcd("0");
        assert!(vcd.contains("$enddefinitions $end"));
        assert!(!vcd.contains("$var"));
    }
}
