//! Real boards over their serial port, through espflash: identify the chip, read the
//! partition table and security eFuses, write images and erase flash.
//!
//! espflash is blocking; run these off the async workers.

use std::path::{Path, PathBuf};

use espflash::connection::{Connection, ResetAfterOperation, ResetBeforeOperation};
use espflash::flasher::Flasher;
use espflash::target::ProgressCallbacks;
use serde::Serialize;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not open {path}: {detail}")]
    Open { path: PathBuf, detail: String },
    #[error("could not talk to the board's bootloader: {0}")]
    Connect(String),
    #[error("{0}")]
    Board(String),
    #[error("the partition table is not valid: {0}")]
    Partitions(String),
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

fn board_err(e: espflash::Error) -> Error {
    Error::Board(e.to_string())
}

/// How to get the chip into its ROM bootloader before talking to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Reset {
    /// USB-Serial-JTAG boards (Espressif VID 303a, PID 1001) get the USB sequence;
    /// everything else the classic DTR/RTS sequence.
    Auto,
    /// The chip is already in download mode (held BOOT, or QEMU with its strap set).
    None,
}

#[derive(Debug, Clone)]
pub struct Options {
    pub port: PathBuf,
    pub reset: Reset,
    /// Load espflash's RAM stub to speed things up. The ROM loader alone works too
    /// (QEMU needs it: the stub does not run there).
    pub use_stub: bool,
}

/// What the ROM and eFuses say about the board.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BoardInfo {
    /// esp32bench.dev chip id where there is one (esp32, esp32s3, ...), else espflash's name.
    pub chip: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
    pub crystal_mhz: u32,
    pub flash_bytes: u64,
    pub features: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mac: Option<String>,
    /// Secure boot and flash encryption, where the ROM reports them (not on the
    /// original ESP32, whose ROM lacks the command).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub security: Option<Security>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Security {
    pub secure_boot: bool,
    pub flash_encryption: bool,
    pub secure_download: bool,
}

/// One partition table entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Partition {
    pub label: String,
    /// "app" or "data", or the number for other types.
    #[serde(rename = "type")]
    pub kind: String,
    pub subtype: String,
    pub offset: u32,
    pub size: u32,
    pub encrypted: bool,
}

/// Where ESP-IDF and the Arduino core put the partition table.
pub const PARTITION_TABLE_OFFSET: u32 = 0x8000;
const PARTITION_TABLE_SIZE: u32 = 0xC00;
const ENTRY_SIZE: usize = 32;

/// Parse a partition table (ESP-IDF binary format: 32-byte entries starting with
/// 0xAA 0x50, ending at an MD5 entry (0xEB 0xEB) or erased flash).
pub fn parse_partition_table(bytes: &[u8]) -> Result<Vec<Partition>, Error> {
    let mut out = Vec::new();
    for entry in bytes.as_chunks::<ENTRY_SIZE>().0 {
        match (entry[0], entry[1]) {
            (0xAA, 0x50) => {}
            (0xEB, 0xEB) | (0xFF, 0xFF) => break,
            (a, b) => {
                return Err(Error::Partitions(format!(
                    "entry {} starts with {a:#04x} {b:#04x}",
                    out.len()
                )));
            }
        }
        let u32_at =
            |i: usize| u32::from_le_bytes([entry[i], entry[i + 1], entry[i + 2], entry[i + 3]]);
        let (kind, subtype) = (entry[2], entry[3]);
        let label_bytes = &entry[12..28];
        let end = label_bytes
            .iter()
            .position(|b| *b == 0)
            .unwrap_or(label_bytes.len());
        out.push(Partition {
            label: String::from_utf8_lossy(&label_bytes[..end]).into_owned(),
            kind: match kind {
                0 => "app".into(),
                1 => "data".into(),
                n => n.to_string(),
            },
            subtype: subtype_name(kind, subtype),
            offset: u32_at(4),
            size: u32_at(8),
            encrypted: u32_at(28) & 1 != 0,
        });
    }
    if out.is_empty() {
        return Err(Error::Partitions(
            "no entries (is the flash erased?)".into(),
        ));
    }
    Ok(out)
}

fn subtype_name(kind: u8, sub: u8) -> String {
    match (kind, sub) {
        (0, 0x00) => "factory".into(),
        (0, 0x10..=0x1F) => format!("ota_{}", sub - 0x10),
        (0, 0x20) => "test".into(),
        (1, 0x00) => "ota".into(),
        (1, 0x01) => "phy".into(),
        (1, 0x02) => "nvs".into(),
        (1, 0x03) => "coredump".into(),
        (1, 0x04) => "nvs_keys".into(),
        (1, 0x05) => "efuse".into(),
        (1, 0x80) => "esphttpd".into(),
        (1, 0x81) => "fat".into(),
        (1, 0x82) => "spiffs".into(),
        (1, 0x83) => "littlefs".into(),
        (_, n) => format!("{n:#04x}"),
    }
}

/// The part of a merged image worth writing: everything up to the last byte that is
/// not erased flash (0xFF), rounded up to a 4 KiB sector. Leaving the rest alone keeps
/// data partitions (saved settings, files) that the image does not cover.
pub fn trim_erased_tail(image: &[u8]) -> &[u8] {
    let used = image.iter().rposition(|b| *b != 0xFF).map_or(0, |i| i + 1);
    let rounded = used.div_ceil(4096) * 4096;
    &image[..rounded.min(image.len())]
}

/// The chip a flash image was built for, from its image header (bootloader at 0x1000
/// for ESP32 and ESP32-S2, at 0 for the others). None if no header is found.
pub fn image_chip(image: &[u8]) -> Option<&'static str> {
    for offset in [0usize, 0x1000] {
        let Some(h) = image.get(offset..offset + 16) else {
            continue;
        };
        if h[0] != 0xE9 {
            continue;
        }
        // Extended header: chip id at byte 12, little-endian.
        let id = u16::from_le_bytes([h[12], h[13]]);
        let chip = match id {
            0 => "esp32",
            2 => "esp32s2",
            5 => "esp32c3",
            9 => "esp32s3",
            12 => "esp32c2",
            13 => "esp32c6",
            16 => "esp32h2",
            18 => "esp32p4",
            _ => continue,
        };
        // The original ESP32 keeps its bootloader at 0x1000; trust a chip id only where
        // that chip's bootloader lives.
        let expected = if matches!(chip, "esp32" | "esp32s2") {
            0x1000
        } else {
            0
        };
        if offset == expected {
            return Some(chip);
        }
    }
    None
}

/// Progress through a write: `written` of `total` bytes at `offset`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Progress {
    Writing {
        offset: u32,
        written: usize,
        total: usize,
    },
    Verifying,
}

struct Callbacks<'a> {
    offset: u32,
    total: usize,
    report: &'a mut dyn FnMut(Progress),
}

impl ProgressCallbacks for Callbacks<'_> {
    fn init(&mut self, addr: u32, total: usize) {
        self.offset = addr;
        self.total = total;
        (self.report)(Progress::Writing {
            offset: addr,
            written: 0,
            total,
        });
    }
    fn update(&mut self, current: usize) {
        (self.report)(Progress::Writing {
            offset: self.offset,
            written: current,
            total: self.total,
        });
    }
    fn verifying(&mut self) {
        (self.report)(Progress::Verifying);
    }
    fn finish(&mut self, _skipped: bool) {}
}

/// A connected board, in its ROM bootloader (or espflash's stub).
pub struct Board {
    flasher: Flasher,
    use_stub: bool,
}

impl Board {
    /// Open the port, reset the chip into its bootloader as `opts.reset` says, and
    /// connect.
    pub fn connect(opts: &Options) -> Result<Self, Error> {
        let name = opts.port.to_string_lossy().into_owned();
        let open_err = |e: serialport::Error| Error::Open {
            path: opts.port.clone(),
            detail: e.to_string(),
        };
        let serial = serialport::new(&name, 115_200)
            .open_native()
            .map_err(open_err)?;
        // espflash picks reset sequences by USB IDs; a pty or unknown device gets zeros.
        let usb = serialport::available_ports()
            .ok()
            .and_then(|ports| ports.into_iter().find(|p| p.port_name == name))
            .and_then(|p| match p.port_type {
                serialport::SerialPortType::UsbPort(u) => Some(u),
                _ => None,
            })
            .unwrap_or(serialport::UsbPortInfo {
                vid: 0,
                pid: 0,
                serial_number: None,
                manufacturer: None,
                product: None,
            });
        let before = match opts.reset {
            Reset::None => ResetBeforeOperation::NoReset,
            Reset::Auto if usb.vid == 0x303a && usb.pid == 0x1001 => ResetBeforeOperation::UsbReset,
            Reset::Auto => ResetBeforeOperation::DefaultReset,
        };
        let after = match opts.reset {
            Reset::None => ResetAfterOperation::NoReset,
            Reset::Auto => ResetAfterOperation::HardReset,
        };
        let connection = Connection::new(serial, usb, after, before, 115_200);
        let flasher = Flasher::connect(connection, opts.use_stub, true, false, None, None)
            .map_err(|e| Error::Connect(e.to_string()))?;
        Ok(Board {
            flasher,
            use_stub: opts.use_stub,
        })
    }

    pub fn info(&mut self) -> Result<BoardInfo, Error> {
        let d = self.flasher.device_info().map_err(board_err)?;
        let security = self.flasher.security_info().ok().map(|s| {
            let flag = |name: &str| s.flags & security_flag(name) != 0;
            Security {
                secure_boot: flag("SECURE_BOOT_EN"),
                // An odd number of bits set in FLASH_CRYPT_CNT means encryption is on.
                flash_encryption: s.flash_crypt_cnt.count_ones() % 2 == 1,
                secure_download: flag("SECURE_DOWNLOAD_ENABLE"),
            }
        });
        Ok(BoardInfo {
            chip: chip_id(d.chip),
            revision: d.revision.map(|(major, minor)| format!("v{major}.{minor}")),
            crystal_mhz: xtal_mhz(&d.crystal_frequency.to_string()),
            flash_bytes: u64::from(d.flash_size.size()),
            features: d.features,
            mac: d.mac_address,
            security,
        })
    }

    /// Read and parse the partition table at the standard offset. `scratch` is a
    /// directory for the temporary file espflash reads into.
    pub fn partitions(&mut self, scratch: &Path) -> Result<Vec<Partition>, Error> {
        let file = scratch.join("partition-table.bin");
        let (offset, size) = (PARTITION_TABLE_OFFSET, PARTITION_TABLE_SIZE);
        // The fast read command exists only in the stub; the ROM has its own.
        let read = if self.use_stub {
            self.flasher
                .read_flash(offset, size, 0x1000, 64, file.clone())
        } else {
            self.flasher
                .read_flash_rom(offset, size, 0x1000, 64, file.clone())
        };
        read.map_err(board_err)?;
        let bytes = std::fs::read(&file)?;
        let _ = std::fs::remove_file(&file);
        parse_partition_table(&bytes)
    }

    /// Write `data` at `offset`. espflash checks the result against the chip's MD5
    /// before returning.
    pub fn write(
        &mut self,
        offset: u32,
        data: &[u8],
        report: &mut dyn FnMut(Progress),
    ) -> Result<(), Error> {
        let mut cb = Callbacks {
            offset,
            total: data.len(),
            report,
        };
        self.flasher
            .write_bin_to_flash(offset, data, &mut cb)
            .map_err(board_err)
    }

    /// Erase the whole flash chip.
    pub fn erase_all(&mut self) -> Result<(), Error> {
        self.flasher.erase_flash().map_err(board_err)
    }

    /// Leave the bootloader and run the firmware (a hard reset), where the reset
    /// strategy allows it.
    pub fn run(self) -> Result<(), Error> {
        let chip = self.flasher.chip();
        let mut conn = self.flasher.into_connection();
        conn.reset_after(false, chip).map_err(board_err)
    }
}

fn security_flag(name: &str) -> u32 {
    match name {
        "SECURE_BOOT_EN" => 1 << 0,
        "SECURE_DOWNLOAD_ENABLE" => 1 << 2,
        _ => 0,
    }
}

/// espflash's chip to the site's chip ids.
fn chip_id(chip: espflash::target::Chip) -> String {
    chip.to_string().to_ascii_lowercase().replace('-', "")
}

fn xtal_mhz(s: &str) -> u32 {
    s.chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(kind: u8, sub: u8, offset: u32, size: u32, label: &str, flags: u32) -> Vec<u8> {
        let mut e = vec![0xAA, 0x50, kind, sub];
        e.extend(offset.to_le_bytes());
        e.extend(size.to_le_bytes());
        let mut l = [0u8; 16];
        l[..label.len()].copy_from_slice(label.as_bytes());
        e.extend(l);
        e.extend(flags.to_le_bytes());
        e
    }

    #[test]
    fn parses_the_default_arduino_table() {
        let mut t = Vec::new();
        t.extend(entry(1, 0x02, 0x9000, 0x5000, "nvs", 0));
        t.extend(entry(1, 0x00, 0xE000, 0x2000, "otadata", 0));
        t.extend(entry(0, 0x10, 0x10000, 0x140000, "app0", 0));
        t.extend(entry(0, 0x11, 0x150000, 0x140000, "app1", 1));
        t.extend(entry(1, 0x82, 0x290000, 0x160000, "spiffs", 0));
        t.extend([0xEB; 32]); // MD5 entry
        t.extend([0xFF; 64]);
        let p = parse_partition_table(&t).unwrap();
        assert_eq!(p.len(), 5);
        assert_eq!(p[0].subtype, "nvs");
        assert_eq!(p[2].kind, "app");
        assert_eq!(p[2].subtype, "ota_0");
        assert_eq!(p[2].offset, 0x10000);
        assert!(p[3].encrypted);
        assert_eq!(p[4].subtype, "spiffs");
        assert_eq!(p[4].size, 0x160000);
    }

    #[test]
    fn rejects_garbage_and_erased_flash() {
        assert!(parse_partition_table(&[0xFF; 64]).is_err());
        assert!(parse_partition_table(&[0x12; 64]).is_err());
    }

    #[test]
    fn trims_erased_tail_to_whole_sectors() {
        let mut img = vec![0xFFu8; 4 * 1024 * 1024];
        img[0x1000] = 0xE9;
        img[0x10000 + 5] = 1;
        assert_eq!(trim_erased_tail(&img).len(), 0x11000);
        assert_eq!(trim_erased_tail(&[0xFF; 100]).len(), 0);
        assert_eq!(trim_erased_tail(&[1, 2, 3]).len(), 3);
    }

    #[test]
    fn reads_the_chip_from_an_image_header() {
        let mut esp32 = vec![0xFFu8; 0x2000];
        esp32[0x1000] = 0xE9;
        esp32[0x100C..0x100E].copy_from_slice(&[0, 0]); // chip id 0
        assert_eq!(image_chip(&esp32), Some("esp32"));
        let mut c3 = vec![0xFFu8; 0x2000];
        c3[0] = 0xE9;
        c3[12] = 5;
        c3[13] = 0;
        assert_eq!(image_chip(&c3), Some("esp32c3"));
        assert_eq!(image_chip(&[0xFF; 0x2000]), None);
    }

    #[test]
    fn reads_crystal_mhz() {
        assert_eq!(xtal_mhz("40 MHz"), 40);
        assert_eq!(xtal_mhz("26MHz"), 26);
    }
}
