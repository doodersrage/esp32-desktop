//! The pin link to `esp32bench-qemu-host`: what the firmware does with bridged GPIOs,
//! and driving them as inputs.
//!
//! Line protocol (see `crates/qemu-host/src/main.rs`): the host sends
//! `out <gpio> <0|1> [<ns>]` when the firmware drives a pin, `in <gpio> <0|1> [<ns>]`
//! when it has driven an input, with the simulation's time in nanoseconds, and
//! `dir <gpio> <0|1>` when a pin's output driver is switched on (1) or off (0); it
//! also sends `i2c ...` lines,
//! ignored here. With a circuit it also sends `part <id> <JSON state>`, `picture <id>
//! <width> <height> <base64 pixels>` and `parterror <id> <message>`. We send
//! `set <gpio> <0|1>` to drive an input and `control <id> <name> <JSON value>` to work
//! a part.

use std::time::Duration;

use crate::link::{Endpoint, OwnedReadHalf, OwnedWriteHalf};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};

use crate::Error;

/// GPIOs the host can report outputs for: the fork reports only GPIO 0 to 31.
pub const MAX_OUTPUT_GPIO: u8 = 31;

/// What the host reports: pins, and the parts of a circuit.
#[derive(Debug, Clone, PartialEq)]
pub enum PinEvent {
    /// The firmware drove `gpio` to `level`, at `time_ns` of simulated time (None from
    /// a host that does not say).
    Level {
        gpio: u8,
        level: bool,
        time_ns: Option<u64>,
    },
    /// The host drove input `gpio` to `level` (for `set` or a part), at `time_ns`.
    Input {
        gpio: u8,
        level: bool,
        time_ns: Option<u64>,
    },
    /// The firmware switched `gpio`'s output driver on (an output) or off (an input).
    Direction { gpio: u8, output: bool },
    /// A part's whole state changed.
    Part {
        id: String,
        state: serde_json::Value,
    },
    /// A display part's picture, one byte per pixel (0 off, 1 on), row by row.
    Picture {
        id: String,
        width: u32,
        height: u32,
        pixels: Vec<u8>,
    },
    /// A part's script failed.
    PartError { id: String, message: String },
    /// The simulation's time, in nanoseconds of QEMU's virtual clock (sent about
    /// twice a second).
    Clock { ns: u64 },
}

/// Parse one line from the host; None for lines that are not pin events.
pub fn parse_line(line: &str) -> Option<PinEvent> {
    if let Some(rest) = line.strip_prefix("part ") {
        let (id, json) = rest.split_once(' ')?;
        return Some(PinEvent::Part {
            id: id.to_owned(),
            state: serde_json::from_str(json).ok()?,
        });
    }
    if let Some(rest) = line.strip_prefix("clock ") {
        return Some(PinEvent::Clock {
            ns: rest.trim().parse().ok()?,
        });
    }
    if let Some(rest) = line.strip_prefix("parterror ") {
        let (id, message) = rest.split_once(' ').unwrap_or((rest, ""));
        return Some(PinEvent::PartError {
            id: id.to_owned(),
            message: message.to_owned(),
        });
    }
    if let Some(rest) = line.strip_prefix("picture ") {
        use base64::Engine;
        let mut it = rest.split(' ');
        let (id, w, h, data) = (it.next()?, it.next()?, it.next()?, it.next()?);
        let (width, height): (u32, u32) = (w.parse().ok()?, h.parse().ok()?);
        let pixels = base64::engine::general_purpose::STANDARD
            .decode(data)
            .ok()?;
        if pixels.len() != (width as usize) * (height as usize) {
            return None;
        }
        return Some(PinEvent::Picture {
            id: id.to_owned(),
            width,
            height,
            pixels,
        });
    }
    let mut it = line.split_ascii_whitespace();
    let kind = it.next()?;
    let gpio: u8 = it.next()?.parse().ok()?;
    let value = match it.next()? {
        "0" => false,
        "1" => true,
        _ => return None,
    };
    let time_ns = match (kind, it.next()) {
        (_, None) => None,
        ("out" | "in", Some(t)) => Some(t.parse().ok()?),
        _ => return None,
    };
    if it.next().is_some() {
        return None;
    }
    match kind {
        "out" => Some(PinEvent::Level {
            gpio,
            level: value,
            time_ns,
        }),
        "in" => Some(PinEvent::Input {
            gpio,
            level: value,
            time_ns,
        }),
        "dir" => Some(PinEvent::Direction {
            gpio,
            output: value,
        }),
        _ => None,
    }
}

pub struct GpioLink {
    lines: Lines<BufReader<OwnedReadHalf>>,
    writer: OwnedWriteHalf,
}

impl GpioLink {
    /// Connect to the host's socket, waiting up to `timeout` for it to appear.
    pub async fn connect(endpoint: &Endpoint, timeout: Duration) -> Result<Self, Error> {
        let deadline = tokio::time::Instant::now() + timeout;
        let stream = loop {
            match endpoint.connect().await {
                Ok(s) => break s,
                Err(e) if tokio::time::Instant::now() >= deadline => return Err(e.into()),
                Err(_) => tokio::time::sleep(Duration::from_millis(20)).await,
            }
        };
        let (r, w) = stream.into_split();
        Ok(GpioLink {
            lines: BufReader::new(r).lines(),
            writer: w,
        })
    }

    /// The next pin event; None when the host has gone.
    pub async fn next_event(&mut self) -> Result<Option<PinEvent>, Error> {
        while let Some(line) = self.lines.next_line().await? {
            if let Some(ev) = parse_line(&line) {
                return Ok(Some(ev));
            }
        }
        Ok(None)
    }

    /// Work a part's control: `value` is a bool or a number.
    pub async fn control(
        &mut self,
        id: &str,
        name: &str,
        value: &serde_json::Value,
    ) -> Result<(), Error> {
        if id.contains(char::is_whitespace) || name.contains(char::is_whitespace) {
            return Err(Error::Gpio("part and control names have no spaces".into()));
        }
        self.writer
            .write_all(format!("control {id} {name} {value}\n").as_bytes())
            .await?;
        Ok(())
    }

    /// Drive `gpio` as an input.
    pub async fn set(&mut self, gpio: u8, level: bool) -> Result<(), Error> {
        self.writer
            .write_all(format!("set {gpio} {}\n", u8::from(level)).as_bytes())
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_host_lines() {
        assert_eq!(
            parse_line("clock 12400000000"),
            Some(PinEvent::Clock { ns: 12_400_000_000 })
        );
        assert_eq!(parse_line("clock soon"), None);
        assert_eq!(
            parse_line("out 2 1"),
            Some(PinEvent::Level {
                gpio: 2,
                level: true,
                time_ns: None
            })
        );
        assert_eq!(
            parse_line("out 2 0 1500"),
            Some(PinEvent::Level {
                gpio: 2,
                level: false,
                time_ns: Some(1500)
            })
        );
        assert_eq!(
            parse_line("in 4 1 2000000"),
            Some(PinEvent::Input {
                gpio: 4,
                level: true,
                time_ns: Some(2_000_000)
            })
        );
        assert_eq!(
            parse_line("dir 4 0"),
            Some(PinEvent::Direction {
                gpio: 4,
                output: false
            })
        );
        assert_eq!(
            parse_line("dir 4 1"),
            Some(PinEvent::Direction {
                gpio: 4,
                output: true
            })
        );
        for other in [
            "i2c 0 3c ack",
            "out 2",
            "out x 1",
            "out 2 7",
            "out 2 1 x",
            "out 2 1 9 9",
            "dir 4 1 9",
            "",
            "set 4 1",
            "picture oled 2 2 AAA=",
            "part led {",
        ] {
            assert_eq!(parse_line(other), None, "{other:?}");
        }
        assert_eq!(
            parse_line(r#"part led-1 {"on":true}"#),
            Some(PinEvent::Part {
                id: "led-1".into(),
                state: serde_json::json!({ "on": true })
            })
        );
        assert_eq!(
            parse_line("picture oled 2 1 AAE="),
            Some(PinEvent::Picture {
                id: "oled".into(),
                width: 2,
                height: 1,
                pixels: vec![0, 1]
            })
        );
        assert_eq!(
            parse_line("parterror go no part `go`"),
            Some(PinEvent::PartError {
                id: "go".into(),
                message: "no part `go`".into()
            })
        );
    }
}
