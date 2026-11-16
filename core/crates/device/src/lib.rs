//! Serial port discovery with a guess at which ESP32 chip is behind each port.
//!
//! The guess comes only from USB IDs. A UART bridge (CP210x, CH340, ...) says nothing
//! about the chip; Espressif's own USB IDs narrow it down. Identifying the chip for
//! sure needs the ROM bootloader, which is phase 2 work.

use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use serde::Serialize;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("listing serial ports failed: {0}")]
    List(String),
}

/// One serial port, as reported by `device.list` (see `schema/rpc/device.list.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Port {
    pub path: String,
    pub kind: Kind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usb: Option<Usb>,
    pub bridge: Option<Bridge>,
    /// esp32bench.dev chip ids this port could belong to.
    pub chips: Vec<&'static str>,
    pub access: Access,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Usb,
    Pci,
    Bluetooth,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Usb {
    pub vid: u16,
    pub pid: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub serial: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manufacturer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub product: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Bridge {
    Cp210x,
    Ch340,
    Ch9102,
    Ftdi,
    /// The chip's built-in USB-Serial-JTAG peripheral.
    UsbSerialJtag,
    /// A USB CDC-ACM port run by the firmware itself (TinyUSB), Espressif VID.
    UsbCdc,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Access {
    Ok,
    Denied,
}

const ESPRESSIF_VID: u16 = 0x303a;

/// What a USB VID:PID says about the board: the bridge type and the possible chips.
pub fn classify(vid: u16, pid: u16) -> (Option<Bridge>, Vec<&'static str>) {
    match (vid, pid) {
        (0x10c4, 0xea60 | 0xea70) => (Some(Bridge::Cp210x), vec![]),
        (0x1a86, 0x7523 | 0x5523) => (Some(Bridge::Ch340), vec![]),
        (0x1a86, 0x55d3 | 0x55d4) => (Some(Bridge::Ch9102), vec![]),
        (0x0403, 0x6001 | 0x6010 | 0x6011 | 0x6014 | 0x6015) => (Some(Bridge::Ftdi), vec![]),
        // USB-Serial-JTAG, built into S3, C3 and C6 (and H2/C5, which the site does not list).
        (ESPRESSIF_VID, 0x1001) => (
            Some(Bridge::UsbSerialJtag),
            vec!["esp32s3", "esp32c3", "esp32c6"],
        ),
        // Any other Espressif PID is the chip's USB OTG peripheral, which only S2 and S3 have.
        (ESPRESSIF_VID, _) => (Some(Bridge::UsbCdc), vec!["esp32s2", "esp32s3"]),
        _ => (None, vec![]),
    }
}

/// Whether this user may open `path` for reading and writing.
pub fn access(path: &Path) -> Access {
    let Ok(c) = CString::new(path.as_os_str().as_bytes()) else {
        return Access::Denied;
    };
    // SAFETY: access(2) with a valid NUL-terminated path; it only reads the string.
    if unsafe { libc::access(c.as_ptr(), libc::R_OK | libc::W_OK) } == 0 {
        Access::Ok
    } else {
        Access::Denied
    }
}

/// List serial ports. Without `all`, only USB ports are returned, since ESP32 boards
/// always connect over USB.
pub fn list(all: bool) -> Result<Vec<Port>, Error> {
    let found = serialport::available_ports().map_err(|e| Error::List(e.to_string()))?;
    let mut ports: Vec<Port> = found
        .into_iter()
        .map(|p| from_info(p.port_name, p.port_type))
        .filter(|p| all || p.kind == Kind::Usb)
        .collect();
    ports.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(ports)
}

fn from_info(path: String, ty: serialport::SerialPortType) -> Port {
    use serialport::SerialPortType as T;
    let (kind, usb) = match ty {
        T::UsbPort(u) => (
            Kind::Usb,
            Some(Usb {
                vid: u.vid,
                pid: u.pid,
                serial: u.serial_number,
                manufacturer: u.manufacturer,
                product: u.product,
            }),
        ),
        T::PciPort => (Kind::Pci, None),
        T::BluetoothPort => (Kind::Bluetooth, None),
        T::Unknown => (Kind::Other, None),
    };
    let (bridge, chips) = usb
        .as_ref()
        .map_or((None, vec![]), |u| classify(u.vid, u.pid));
    let access = access(Path::new(&path));
    Port {
        path,
        kind,
        usb,
        bridge,
        chips,
        access,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_common_bridges_without_guessing_the_chip() {
        assert_eq!(classify(0x10c4, 0xea60), (Some(Bridge::Cp210x), vec![]));
        assert_eq!(classify(0x1a86, 0x7523), (Some(Bridge::Ch340), vec![]));
        assert_eq!(classify(0x1a86, 0x55d4), (Some(Bridge::Ch9102), vec![]));
        assert_eq!(classify(0x0403, 0x6001), (Some(Bridge::Ftdi), vec![]));
        assert_eq!(classify(0x046d, 0xc52b), (None, vec![]));
    }

    #[test]
    fn narrows_chips_from_espressif_ids() {
        let (b, chips) = classify(0x303a, 0x1001);
        assert_eq!(b, Some(Bridge::UsbSerialJtag));
        assert_eq!(chips, ["esp32s3", "esp32c3", "esp32c6"]);
        let (b, chips) = classify(0x303a, 0x0002);
        assert_eq!(b, Some(Bridge::UsbCdc));
        assert_eq!(chips, ["esp32s2", "esp32s3"]);
    }

    #[test]
    fn serializes_to_the_schema_shape() {
        let p = from_info(
            "/nonexistent/ttyACM9".into(),
            serialport::SerialPortType::UsbPort(serialport::UsbPortInfo {
                vid: 0x303a,
                pid: 0x1001,
                serial_number: Some("F4:12".into()),
                manufacturer: None,
                product: None,
            }),
        );
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(v["bridge"], "usb-serial-jtag");
        assert_eq!(v["access"], "denied");
        assert_eq!(v["usb"]["vid"], 0x303a);
        assert!(v["usb"].get("manufacturer").is_none());

        let p = from_info("/dev/ttyS0".into(), serialport::SerialPortType::Unknown);
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(v["kind"], "other");
        assert!(v["bridge"].is_null());
        assert!(v.get("usb").is_none());
    }
}
