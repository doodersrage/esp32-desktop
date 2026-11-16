//! QEMU supervision for the esp32-desktop simulator.
//!
//! The simulator is firmware-level: it runs ESP32, ESP32-S3 and ESP32-C3 images in
//! Espressif's QEMU fork. It does not model circuits.

pub mod chip;
pub mod error;
pub mod gdb;
pub mod image;
pub mod install;
pub mod lcgamboa;
pub mod pin;
pub mod qmp;
pub mod session;
pub mod uart;

pub use chip::{Arch, Chip};
pub use error::Error;
pub use session::{Session, SessionConfig};
