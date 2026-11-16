//! Merged flash images: inspection and padding to a size QEMU accepts.

use serde::Serialize;

use crate::{Chip, Error};

const MIB: usize = 1024 * 1024;

/// Flash sizes QEMU's ESP32 machines accept for an `if=mtd` drive.
pub const FLASH_SIZES: [usize; 4] = [2 * MIB, 4 * MIB, 8 * MIB, 16 * MIB];

const IMAGE_MAGIC: u8 = 0xE9;

/// SPI flash mode from the bootloader image header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FlashMode {
    Qio,
    Qout,
    Dio,
    Dout,
    Unknown(u8),
}

impl FlashMode {
    fn from_byte(b: u8) -> Self {
        match b {
            0 => FlashMode::Qio,
            1 => FlashMode::Qout,
            2 => FlashMode::Dio,
            3 => FlashMode::Dout,
            other => FlashMode::Unknown(other),
        }
    }

    /// QEMU only boots DIO and DOUT images.
    pub fn boots_in_qemu(self) -> bool {
        matches!(self, FlashMode::Dio | FlashMode::Dout)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ImageInfo {
    pub chip: Chip,
    pub len: usize,
    /// Flash size the bootloader header declares, if it is one QEMU emulates.
    pub declared_flash_size: Option<usize>,
    /// Size the image is padded to: the declared size, or larger if the data needs it.
    pub flash_size: usize,
    pub flash_mode: FlashMode,
}

/// Check that `data` is a merged image for `chip` and report what QEMU will see.
pub fn inspect(chip: Chip, data: &[u8]) -> Result<ImageInfo, Error> {
    let off = chip.bootloader_offset();
    let header = data
        .get(off..off + 4)
        .ok_or(Error::NotMergedImage { chip, offset: off })?;
    if header[0] != IMAGE_MAGIC {
        return Err(Error::NotMergedImage { chip, offset: off });
    }
    let declared = declared_flash_size(header[3]);
    let needed = flash_size_for(data.len())?;
    Ok(ImageInfo {
        chip,
        len: data.len(),
        declared_flash_size: declared,
        flash_size: declared.map_or(needed, |d| d.max(needed)),
        flash_mode: FlashMode::from_byte(header[2]),
    })
}

/// Decode the size nibble of the header's size/frequency byte (1, 2, 4, 8 or 16 MiB).
/// 1 MiB is not a size QEMU emulates, so it is treated as undeclared.
fn declared_flash_size(b: u8) -> Option<usize> {
    match b >> 4 {
        1 => Some(2 * MIB),
        2 => Some(4 * MIB),
        3 => Some(8 * MIB),
        4 => Some(16 * MIB),
        _ => None,
    }
}

/// Smallest flash size that holds `len` bytes.
pub fn flash_size_for(len: usize) -> Result<usize, Error> {
    FLASH_SIZES
        .iter()
        .copied()
        .find(|s| *s >= len)
        .ok_or(Error::ImageTooLarge(len))
}

/// Pad a merged image with 0xFF (erased flash) to `info.flash_size`.
pub fn pad(mut data: Vec<u8>, info: &ImageInfo) -> Vec<u8> {
    data.resize(info.flash_size.max(data.len()), 0xFF);
    data
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_image(chip: Chip, mode: u8, len: usize) -> Vec<u8> {
        let mut v = vec![0xFF; len];
        let off = chip.bootloader_offset();
        v[off..off + 4].copy_from_slice(&[IMAGE_MAGIC, 3, mode, 0x20]);
        v
    }

    fn padded_len(chip: Chip, size_byte: u8, len: usize) -> usize {
        let mut img = fake_image(chip, 2, len);
        let off = chip.bootloader_offset();
        img[off + 3] = size_byte;
        let info = inspect(chip, &img).unwrap();
        let padded = pad(img, &info);
        assert_eq!(padded[len], 0xFF);
        padded.len()
    }

    #[test]
    fn pads_to_declared_flash_size() {
        // 0x20 declares 4 MiB, as the esp32-bench images do.
        assert_eq!(padded_len(Chip::Esp32, 0x20, 337_872), 4 * MIB);
        assert_eq!(padded_len(Chip::Esp32c3, 0x3f, 337_872), 8 * MIB);
    }

    #[test]
    fn pads_to_fit_when_header_is_too_small() {
        // 0x00 declares 1 MiB, which QEMU does not emulate.
        assert_eq!(padded_len(Chip::Esp32, 0x00, 337_872), 2 * MIB);
        // Declared 2 MiB but the data is larger.
        assert_eq!(padded_len(Chip::Esp32, 0x10, 2 * MIB + 1), 4 * MIB);
    }

    #[test]
    fn rejects_oversize_image() {
        let img = fake_image(Chip::Esp32, 2, 16 * MIB + 1);
        assert!(matches!(
            inspect(Chip::Esp32, &img),
            Err(Error::ImageTooLarge(_))
        ));
    }

    #[test]
    fn reads_flash_mode_at_chip_offset() {
        let info = inspect(Chip::Esp32, &fake_image(Chip::Esp32, 2, 8192)).unwrap();
        assert_eq!(info.flash_mode, FlashMode::Dio);
        assert!(info.flash_mode.boots_in_qemu());

        let info = inspect(Chip::Esp32c3, &fake_image(Chip::Esp32c3, 0, 8192)).unwrap();
        assert_eq!(info.flash_mode, FlashMode::Qio);
        assert!(!info.flash_mode.boots_in_qemu());
    }

    #[test]
    fn rejects_app_only_or_wrong_chip_image() {
        // An ESP32-S3 image has its bootloader at 0, not 0x1000.
        let s3 = fake_image(Chip::Esp32s3, 2, 8192);
        assert!(matches!(
            inspect(Chip::Esp32, &s3),
            Err(Error::NotMergedImage { .. })
        ));
        assert!(matches!(
            inspect(Chip::Esp32, &[0xE9]),
            Err(Error::NotMergedImage { .. })
        ));
    }
}
