//! QEMU's virtual RGB panel (`esp_rgb`), the display firmware draws on with
//! Espressif's `esp_lcd_qemu_rgb` driver, read through QMP `screendump`.
//!
//! The panel only finishes a draw when a display refreshes it: the driver spins until
//! the device says the update is done, and with `-display none` nothing refreshes. So
//! a session must be polled for as long as it runs, or firmware that draws hangs.
//! Each poll is one screendump (about 1 ms for 320x240) that refreshes the panel and
//! returns the picture.
//!
//! An unconfigured panel is QEMU's default 800x600 in one colour; frames are reported
//! only once the picture differs from that, so firmware without a display shows none.
//! The panel is a QEMU device: real displays (SSD1306, ST7789...) are not drawn here.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::json;

use crate::Error;
use crate::qmp::Qmp;

/// Poll interval while the picture is changing.
pub const FAST: Duration = Duration::from_millis(40);
/// Poll interval otherwise: still refreshes the panel, so a first draw waits at most this.
pub const SLOW: Duration = Duration::from_millis(100);
/// How long without a change before polling slows down.
const SETTLE: Duration = Duration::from_secs(2);

const DEFAULT_SIZE: (u32, u32) = (800, 600);

/// One picture, 8-bit RGB, rows top to bottom.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
}

impl Frame {
    /// QEMU's picture of a panel no firmware has set up: 800x600, one colour.
    pub fn is_unconfigured(&self) -> bool {
        (self.width, self.height) == DEFAULT_SIZE
            && self
                .rgb
                .as_chunks::<3>()
                .0
                .iter()
                .all(|p| p[..] == self.rgb[..3])
    }

    /// The colour at (x, y).
    pub fn pixel(&self, x: u32, y: u32) -> Option<[u8; 3]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let i = usize::try_from((y * self.width + x) * 3).ok()?;
        self.rgb.get(i..i + 3).map(|p| [p[0], p[1], p[2]])
    }
}

/// Parse a binary PPM (P6, maxval 255), as `screendump` writes.
pub fn parse_ppm(data: &[u8]) -> Option<Frame> {
    let mut fields = Vec::new();
    let mut i = 0;
    while fields.len() < 4 {
        // Skip whitespace and comments.
        while let Some(&c) = data.get(i) {
            if c == b'#' {
                while data.get(i).is_some_and(|&c| c != b'\n') {
                    i += 1;
                }
            } else if c.is_ascii_whitespace() {
                i += 1;
            } else {
                break;
            }
        }
        let start = i;
        while data.get(i).is_some_and(|c| !c.is_ascii_whitespace()) {
            i += 1;
        }
        if start == i {
            return None;
        }
        fields.push(std::str::from_utf8(&data[start..i]).ok()?);
    }
    // Exactly one whitespace byte separates the header from the pixels.
    i += 1;
    if fields[0] != "P6" || fields[3] != "255" {
        return None;
    }
    let width: u32 = fields[1].parse().ok()?;
    let height: u32 = fields[2].parse().ok()?;
    let len = usize::try_from(u64::from(width) * u64::from(height) * 3).ok()?;
    let rgb = data.get(i..i.checked_add(len)?)?.to_vec();
    Some(Frame { width, height, rgb })
}

/// Polls one session's panel.
pub struct Display {
    path: PathBuf,
    last: Option<Frame>,
    in_use: bool,
    changed_at: Instant,
}

impl Display {
    /// Screendumps go to `dir` (the session's private directory).
    pub fn new(dir: &Path) -> Self {
        Display {
            path: dir.join("screen.ppm"),
            last: None,
            in_use: false,
            changed_at: Instant::now(),
        }
    }

    /// How long to wait before the next poll.
    pub fn interval(&self) -> Duration {
        if self.in_use && self.changed_at.elapsed() < SETTLE {
            FAST
        } else {
            SLOW
        }
    }

    /// Whether firmware has set up the panel.
    pub fn in_use(&self) -> bool {
        self.in_use
    }

    /// Refresh the panel and return its picture if it changed and is in use.
    pub async fn poll(&mut self, qmp: &mut Qmp) -> Result<Option<Frame>, Error> {
        qmp.execute("screendump", Some(json!({ "filename": self.path })))
            .await?;
        let data = tokio::fs::read(&self.path).await?;
        let frame = parse_ppm(&data).ok_or(Error::BadScreendump)?;
        if self.last.as_ref() == Some(&frame) {
            return Ok(None);
        }
        self.changed_at = Instant::now();
        if !frame.is_unconfigured() {
            self.in_use = true;
        }
        self.last = Some(frame.clone());
        Ok(self.in_use.then_some(frame))
    }
}

/// A picture as PNG: `rgb` is `width * height` pixels of red, green and blue, rows
/// top to bottom (as sim.frame carries them). None if the sizes do not match.
pub fn png(width: u32, height: u32, rgb: &[u8]) -> Option<Vec<u8>> {
    use std::io::Write;
    let row = usize::try_from(width).ok()?.checked_mul(3)?;
    if width == 0 || height == 0 || rgb.len() != row.checked_mul(usize::try_from(height).ok()?)? {
        return None;
    }
    let chunk = |out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]| {
        out.extend_from_slice(&u32::try_from(data.len()).unwrap_or(u32::MAX).to_be_bytes());
        let mut crc = crc32fast::Hasher::new();
        crc.update(kind);
        crc.update(data);
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        out.extend_from_slice(&crc.finalize().to_be_bytes());
    };
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]); // 8 bits, RGB, deflate, no filter, no interlace
    chunk(&mut out, b"IHDR", &ihdr);
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    for line in rgb.chunks(row) {
        z.write_all(&[0]).ok()?; // filter: none
        z.write_all(line).ok()?;
    }
    chunk(&mut out, b"IDAT", &z.finish().ok()?);
    chunk(&mut out, b"IEND", &[]);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ppm(w: u32, h: u32, px: [u8; 3]) -> Vec<u8> {
        let mut v = format!("P6\n# comment\n{w} {h}\n255\n").into_bytes();
        for _ in 0..w * h {
            v.extend_from_slice(&px);
        }
        v
    }

    #[test]
    fn parses_qemu_ppm() {
        let f = parse_ppm(&ppm(3, 2, [1, 2, 3])).unwrap();
        assert_eq!((f.width, f.height), (3, 2));
        assert_eq!(f.pixel(2, 1), Some([1, 2, 3]));
        assert_eq!(f.pixel(3, 0), None);
        // A pixel value of 10 (a newline byte) right after the header is data.
        let f = parse_ppm(b"P6 1 1 255\n\n\x0a\x0a").unwrap();
        assert_eq!(f.rgb, [10, 10, 10]);
    }

    #[test]
    fn rejects_other_formats_and_short_data() {
        assert!(parse_ppm(b"P5 1 1 255\n\0").is_none());
        assert!(parse_ppm(b"P6 2 2 255\n\0\0\0").is_none());
        assert!(parse_ppm(b"P6 1 1 65535\n\0\0\0\0\0\0").is_none());
        assert!(parse_ppm(b"").is_none());
    }

    #[test]
    fn untouched_panel_is_unconfigured() {
        assert!(
            parse_ppm(&ppm(800, 600, [0, 0, 0]))
                .unwrap()
                .is_unconfigured()
        );
        assert!(
            !parse_ppm(&ppm(320, 240, [0, 0, 0]))
                .unwrap()
                .is_unconfigured()
        );
        let mut drawn = ppm(800, 600, [0, 0, 0]);
        let n = drawn.len();
        drawn[n - 1] = 255;
        assert!(!parse_ppm(&drawn).unwrap().is_unconfigured());
    }
}

#[cfg(test)]
mod png_tests {
    #[test]
    fn writes_a_png() {
        let px = [255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255];
        let png = super::png(2, 2, &px).unwrap();
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
        assert_eq!(&png[12..16], b"IHDR");
        assert!(png.ends_with(&[0xae, 0x42, 0x60, 0x82])); // IEND's CRC
        // The pixels come back out of the zlib stream with one filter byte a row.
        let idat = png.windows(4).position(|w| w == b"IDAT").unwrap();
        let len = u32::from_be_bytes(png[idat - 4..idat].try_into().unwrap()) as usize;
        let mut raw = Vec::new();
        use std::io::Read;
        flate2::read::ZlibDecoder::new(&png[idat + 4..idat + 4 + len])
            .read_to_end(&mut raw)
            .unwrap();
        assert_eq!(raw, [0, 255, 0, 0, 0, 255, 0, 0, 0, 0, 255, 255, 255, 255]);
        assert!(super::png(2, 2, &px[..9]).is_none());
        assert!(super::png(0, 2, &[]).is_none());
    }
}
