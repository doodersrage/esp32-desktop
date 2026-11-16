//! Chips the simulator supports, and how each maps onto QEMU.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::Error;

/// A chip that runs in QEMU. Other chips are workbench-only and have no variant here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Chip {
    Esp32,
    Esp32s3,
    Esp32c3,
}

/// The QEMU system emulator a chip needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Arch {
    Xtensa,
    Riscv32,
}

impl Chip {
    pub const ALL: [Chip; 3] = [Chip::Esp32, Chip::Esp32s3, Chip::Esp32c3];

    pub fn arch(self) -> Arch {
        match self {
            Chip::Esp32 | Chip::Esp32s3 => Arch::Xtensa,
            Chip::Esp32c3 => Arch::Riscv32,
        }
    }

    /// Value for QEMU's `-machine` option.
    pub fn machine(self) -> &'static str {
        match self {
            Chip::Esp32 => "esp32",
            Chip::Esp32s3 => "esp32s3",
            Chip::Esp32c3 => "esp32c3",
        }
    }

    /// Flash offset of the second-stage bootloader in a merged image.
    pub fn bootloader_offset(self) -> usize {
        match self {
            Chip::Esp32 => 0x1000,
            Chip::Esp32s3 | Chip::Esp32c3 => 0x0,
        }
    }
}

impl Arch {
    pub const ALL: [Arch; 2] = [Arch::Xtensa, Arch::Riscv32];

    pub fn binary_name(self) -> &'static str {
        match self {
            Arch::Xtensa => "qemu-system-xtensa",
            Arch::Riscv32 => "qemu-system-riscv32",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Arch::Xtensa => "xtensa",
            Arch::Riscv32 => "riscv32",
        }
    }
}

impl fmt::Display for Chip {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.machine())
    }
}

impl fmt::Display for Arch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Chip {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let norm: String = s
            .chars()
            .filter(|c| *c != '-' && *c != '_')
            .collect::<String>()
            .to_ascii_lowercase();
        match norm.as_str() {
            "esp32" => Ok(Chip::Esp32),
            "esp32s3" => Ok(Chip::Esp32s3),
            "esp32c3" => Ok(Chip::Esp32c3),
            "esp32s2" | "esp32c6" | "esp32h2" | "esp32c2" | "esp32p4" | "esp32c5" => {
                Err(Error::NotSimulated(s.to_owned()))
            }
            _ => Err(Error::UnknownChip(s.to_owned())),
        }
    }
}

impl FromStr for Arch {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "xtensa" => Ok(Arch::Xtensa),
            "riscv32" => Ok(Arch::Riscv32),
            _ => Err(Error::UnknownArch(s.to_owned())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_simulated_chips_in_several_spellings() {
        assert_eq!("esp32".parse::<Chip>().unwrap(), Chip::Esp32);
        assert_eq!("ESP32-S3".parse::<Chip>().unwrap(), Chip::Esp32s3);
        assert_eq!("esp32_c3".parse::<Chip>().unwrap(), Chip::Esp32c3);
    }

    #[test]
    fn rejects_workbench_only_chips() {
        assert!(matches!(
            "esp32-s2".parse::<Chip>(),
            Err(Error::NotSimulated(_))
        ));
        assert!(matches!(
            "esp32c6".parse::<Chip>(),
            Err(Error::NotSimulated(_))
        ));
        assert!(matches!(
            "atmega328".parse::<Chip>(),
            Err(Error::UnknownChip(_))
        ));
    }

    #[test]
    fn maps_chips_to_qemu() {
        assert_eq!(Chip::Esp32c3.arch().binary_name(), "qemu-system-riscv32");
        assert_eq!(Chip::Esp32s3.arch().binary_name(), "qemu-system-xtensa");
        assert_eq!(Chip::Esp32.bootloader_offset(), 0x1000);
    }
}
