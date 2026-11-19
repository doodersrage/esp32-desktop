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
