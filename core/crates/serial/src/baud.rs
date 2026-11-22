//! Finding a board's baud rate: the usual speeds, tried in turn, scored by how much
//! of what arrives reads as text. Output at the wrong speed is mostly control
//! characters and broken UTF-8 (see `style::Garble`).

/// The speeds to try, the likeliest first: Arduino-ESP32 and ESP-IDF default to
/// 115200, the ROM prints at 115200 (74880 on the ESP8266), older sketches 9600.
pub const CANDIDATES: [u32; 9] = [
    115_200, 9_600, 74_880, 57_600, 230_400, 38_400, 19_200, 460_800, 921_600,
];

/// Characters needed to judge a speed.
pub const ENOUGH: usize = 48;

/// The share of characters that read as text (printable ASCII, whitespace, and
/// other letters), and how many characters there were; None with too few to judge.
pub fn readability(bytes: &[u8]) -> Option<(f64, usize)> {
    let text = String::from_utf8_lossy(bytes);
    let (mut seen, mut good) = (0usize, 0usize);
    for ch in text.chars() {
        seen += 1;
        let readable = (ch.is_ascii_graphic() || matches!(ch, ' ' | '\n' | '\r' | '\t' | '\u{1b}'))
            || (ch.is_alphanumeric() && ch != '\u{fffd}');
        if readable {
            good += 1;
        }
    }
    (seen >= ENOUGH).then(|| (good as f64 / seen as f64, seen))
}

/// The speed to use from what each one gave: the first that read as text (95%), else
/// the best above 85%; None if none did.
pub fn choose(tried: &[(u32, Option<(f64, usize)>)]) -> Option<u32> {
    if let Some((baud, _)) = tried
        .iter()
        .find(|(_, r)| r.is_some_and(|(share, _)| share >= 0.95))
    {
        return Some(*baud);
    }
    tried
        .iter()
        .filter_map(|(baud, r)| r.map(|(share, _)| (*baud, share)))
        .filter(|(_, share)| *share >= 0.85)
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(baud, _)| baud)
}

/// What one speed gave: its readability (see [`readability`]).
pub type Tried = (u32, Option<(f64, usize)>);

/// Try each of [`CANDIDATES`] on the port at `path` (resetting the board at each if
/// `reset`), listening a second and a half at each; stop at the first that clearly
/// reads as text. Blocking: call it off the async workers. The speed chosen (see
/// [`choose`]) and what each gave.
pub fn find(
    path: &std::path::Path,
    reset: bool,
) -> Result<(Option<u32>, Vec<Tried>), crate::Error> {
    let mut tried: Vec<Tried> = Vec::new();
    for baud in CANDIDATES {
        // The port closes as its last reader ends (the monitor, or the last speed):
        // wait a moment for it.
        let mut attempt = 0;
        let (mut port, mut rx) = loop {
            match crate::Port::open(path, baud) {
                Err(crate::Error::Busy(_)) if attempt < 20 => {
                    attempt += 1;
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                other => break other?,
            }
        };
        if reset {
            port.reset()?;
        }
        let until = std::time::Instant::now() + std::time::Duration::from_millis(1500);
        let mut bytes = Vec::new();
        while bytes.len() < 512 && std::time::Instant::now() < until {
            match rx.try_recv() {
                Ok(crate::Event::Data(b)) => bytes.extend_from_slice(&b),
                Ok(crate::Event::Closed(_)) => break,
                Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => break,
            }
        }
        drop(port);
        let r = readability(&bytes);
        tried.push((baud, r));
        if r.is_some_and(|(share, _)| share >= 0.95) {
            return Ok((Some(baud), tried));
        }
    }
    Ok((choose(&tried), tried))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_reads_and_noise_does_not() {
        let text = b"ets Jul 29 2019 12:21:46\r\nrst:0x1 (POWERON_RESET),boot:0x13\r\nhello\r\n";
        assert!(readability(text).unwrap().0 > 0.99);
        let noise: Vec<u8> = (0..200u32)
            .map(|i| [0x80u8, 0xfe, 0x13, 0x02, 0xc3][i as usize % 5])
            .collect();
        assert!(readability(&noise).unwrap().0 < 0.5);
        assert_eq!(readability(b"hi"), None);
    }

    #[test]
    fn chooses_the_first_clear_speed_else_the_best() {
        let clear = Some((0.99, 100));
        let fair = Some((0.9, 100));
        let bad = Some((0.3, 100));
        assert_eq!(
            choose(&[(115_200, bad), (9_600, clear), (74_880, clear)]),
            Some(9_600)
        );
        assert_eq!(
            choose(&[(115_200, bad), (9_600, fair), (74_880, Some((0.88, 100)))]),
            Some(9_600)
        );
        assert_eq!(choose(&[(115_200, bad), (9_600, None)]), None);
    }
}
