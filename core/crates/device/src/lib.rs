//! Serial port discovery with a guess at which ESP32 chip is behind each port.
//!
//! The guess comes only from USB IDs. A UART bridge (CP210x, CH340, ...) says nothing
//! about the chip; Espressif's own USB IDs narrow it down. Identifying the chip for
//! sure needs the ROM bootloader, which is phase 2 work.

use std::path::Path;

use serde::Serialize;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("listing serial ports failed: {0}")]
    List(String),
    #[error("watching for device changes failed: {0}")]
    Monitor(String),
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
    /// `chips` is the chip a board check found on this adapter before (`Known`),
    /// not a guess from USB IDs.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub identified: bool,
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
    /// Prolific PL2303 (old USB-UART cables and modules).
    Pl2303,
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

/// USB devices esp32bench recognises: vendor, product (None for any), bridge and the
/// chips it could be. packaging/udev/69-esp32bench.rules must grant access to every
/// entry; a test keeps the two in step.
pub const KNOWN_USB: &[(u16, Option<u16>, Bridge, &[&str])] = &[
    (0x10c4, Some(0xea60), Bridge::Cp210x, &[]),
    (0x10c4, Some(0xea70), Bridge::Cp210x, &[]),
    (0x1a86, Some(0x7523), Bridge::Ch340, &[]),
    (0x1a86, Some(0x5523), Bridge::Ch340, &[]),
    (0x1a86, Some(0x55d3), Bridge::Ch9102, &[]),
    (0x1a86, Some(0x55d4), Bridge::Ch9102, &[]),
    (0x0403, Some(0x6001), Bridge::Ftdi, &[]),
    (0x0403, Some(0x6010), Bridge::Ftdi, &[]),
    (0x0403, Some(0x6011), Bridge::Ftdi, &[]),
    (0x0403, Some(0x6014), Bridge::Ftdi, &[]),
    (0x0403, Some(0x6015), Bridge::Ftdi, &[]),
    (0x067b, Some(0x2303), Bridge::Pl2303, &[]),
    // USB-Serial-JTAG, built into S3, C3 and C6 (and H2/C5, which the site does not list).
    (
        ESPRESSIF_VID,
        Some(0x1001),
        Bridge::UsbSerialJtag,
        &["esp32s3", "esp32c3", "esp32c6"],
    ),
    // Any other Espressif PID is the chip's USB OTG peripheral, which only S2 and S3 have.
    (ESPRESSIF_VID, None, Bridge::UsbCdc, &["esp32s2", "esp32s3"]),
];

/// What a USB VID:PID says about the board: the bridge type and the possible chips.
pub fn classify(vid: u16, pid: u16) -> (Option<Bridge>, Vec<&'static str>) {
    KNOWN_USB
        .iter()
        .find(|(v, p, _, _)| *v == vid && p.is_none_or(|p| p == pid))
        .map_or((None, vec![]), |(_, _, b, chips)| {
            (Some(*b), chips.to_vec())
        })
}

/// Whether this user may open `path` for reading and writing. On Windows there is no
/// such check short of opening the port, which would take it from whoever has it, so
/// every port counts as accessible there; opening one says if it is not.
#[cfg(windows)]
pub fn access(_path: &Path) -> Access {
    Access::Ok
}

/// Whether this user may open `path` for reading and writing.
#[cfg(unix)]
pub fn access(path: &Path) -> Access {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return Access::Denied;
    };
    // SAFETY: access(2) with a valid NUL-terminated path; it only reads the string.
    if unsafe { libc::access(c.as_ptr(), libc::R_OK | libc::W_OK) } == 0 {
        Access::Ok
    } else {
        Access::Denied
    }
}

/// Why a board may be missing or unusable, for the app to put in its own words.
/// See `advice` in `schema/rpc/device.list.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Advice {
    /// No port at all: a charge-only USB cable shows nothing, so try a data cable.
    DataCable,
    /// macOS and Windows: boards with a CH340 or CP210x USB chip may need its maker's
    /// driver before a port appears.
    InstallDriver,
    /// A USB serial chip is plugged in but has no serial port, so no driver took it
    /// (on Linux, often brltty claiming a CH340).
    BridgeWithoutPort { bridge: Bridge },
    /// A port this user may not open (the udev rule or the serial group is missing).
    SerialAccess,
}

/// The advice for these ports, from what else is plugged in on this computer.
pub fn advice(ports: &[Port]) -> Vec<Advice> {
    advice_from(ports, &usb_devices(), std::env::consts::OS)
}

/// The advice for `ports`, given the USB devices plugged in (VID, PID) and the
/// operating system (`std::env::consts::OS`).
pub fn advice_from(ports: &[Port], usb: &[(u16, u16)], os: &str) -> Vec<Advice> {
    let mut advice = Vec::new();
    for &(vid, pid) in usb {
        let (Some(bridge), _) = classify(vid, pid) else {
            continue;
        };
        // A firmware's own USB device need not be a serial port.
        if bridge == Bridge::UsbCdc {
            continue;
        }
        let has_port = ports
            .iter()
            .any(|p| p.usb.as_ref().is_some_and(|u| u.vid == vid && u.pid == pid));
        let a = Advice::BridgeWithoutPort { bridge };
        if !has_port && !advice.contains(&a) {
            advice.push(a);
        }
    }
    if ports.iter().any(|p| p.access == Access::Denied) {
        advice.push(Advice::SerialAccess);
    }
    if ports.is_empty() && advice.is_empty() {
        advice.push(Advice::DataCable);
        if matches!(os, "macos" | "windows") {
            advice.push(Advice::InstallDriver);
        }
    }
    advice
}

/// The USB devices plugged in, as (VID, PID). Only Linux lists them (from sysfs);
/// elsewhere this is empty.
fn usb_devices() -> Vec<(u16, u16)> {
    #[cfg(target_os = "linux")]
    {
        let hex = |dir: &Path, name: &str| -> Option<u16> {
            let s = std::fs::read_to_string(dir.join(name)).ok()?;
            u16::from_str_radix(s.trim(), 16).ok()
        };
        let Ok(entries) = std::fs::read_dir("/sys/bus/usb/devices") else {
            return Vec::new();
        };
        entries
            .filter_map(|e| {
                let dir = e.ok()?.path();
                Some((hex(&dir, "idVendor")?, hex(&dir, "idProduct")?))
            })
            .collect()
    }
    #[cfg(not(target_os = "linux"))]
    {
        Vec::new()
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
        identified: false,
    }
}

/// The chips board checks found behind USB serial adapters, by the adapter's USB
/// IDs and serial number, in a JSON file in the data directory (`boards.json`).
/// Adapters with no serial number (many CH340s) cannot be told apart, so none is
/// remembered for them.
#[derive(Debug, Clone)]
pub struct Known {
    file: std::path::PathBuf,
}

/// Chip ids a port may name (as in `schema/rpc/device.list.json`).
const CHIP_IDS: &[&str] = &["esp32", "esp32s2", "esp32s3", "esp32c3", "esp32c6"];

impl Known {
    pub fn new(file: std::path::PathBuf) -> Self {
        Known { file }
    }

    fn all(&self) -> std::collections::BTreeMap<String, String> {
        std::fs::read(&self.file)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    /// The key an adapter is remembered by, if it has a serial number.
    pub fn key(usb: &Usb) -> Option<String> {
        let serial = usb.serial.as_deref().filter(|s| !s.trim().is_empty())?;
        Some(format!("{:04x}:{:04x}:{serial}", usb.vid, usb.pid))
    }

    /// Remember that `chip` is behind the adapter at `port`. False if it cannot be
    /// remembered (no serial number, or a chip ports do not name).
    pub fn remember(&self, port: &Port, chip: &str) -> std::io::Result<bool> {
        let Some(key) = port.usb.as_ref().and_then(Known::key) else {
            return Ok(false);
        };
        if !CHIP_IDS.contains(&chip) {
            return Ok(false);
        }
        let mut all = self.all();
        all.insert(key, chip.to_owned());
        if let Some(dir) = self.file.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = self.file.with_extension("tmp");
        std::fs::write(
            &tmp,
            serde_json::to_vec_pretty(&all).map_err(std::io::Error::other)?,
        )?;
        std::fs::rename(tmp, &self.file)?;
        Ok(true)
    }

    /// Fill in the chips found before, for ports whose USB IDs do not say.
    pub fn apply(&self, ports: &mut [Port]) {
        let all = self.all();
        for p in ports {
            if p.chips.len() == 1 {
                continue;
            }
            let Some(chip) = p
                .usb
                .as_ref()
                .and_then(Known::key)
                .and_then(|k| all.get(&k))
                .and_then(|c| CHIP_IDS.iter().find(|x| **x == c.as_str()))
            else {
                continue;
            };
            p.chips = vec![*chip];
            p.identified = true;
        }
    }
}

/// Waits for serial ports to appear or disappear: on Linux with a udev monitor on the
/// `tty` subsystem and USB devices, elsewhere by listing the ports every second. Either way the watch
/// lives on its own thread (libudev handles are not thread-safe) and signals changes
/// over a channel; that thread ends within a second of the `Monitor` being dropped.
pub struct Monitor {
    rx: tokio::sync::mpsc::UnboundedReceiver<()>,
}

/// How long to wait for a burst of udev events (one plug-in sends several) to settle.
const SETTLE: std::time::Duration = std::time::Duration::from_millis(300);
/// How often the monitor thread checks whether anyone is still listening.
#[cfg(target_os = "linux")]
const POLL_MS: libc::c_int = 500;

impl Monitor {
    pub fn new() -> Result<Self, Error> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();
        std::thread::Builder::new()
            .name("port-monitor".into())
            .spawn(move || monitor_thread(tx, ready_tx))
            .map_err(|e| Error::Monitor(e.to_string()))?;
        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Monitor { rx }),
            Ok(Err(e)) => Err(Error::Monitor(e)),
            Err(_) => Err(Error::Monitor("monitor thread exited".into())),
        }
    }

    /// Wait until a serial port is added or removed, then until the burst of events
    /// settles. The caller re-lists the ports afterwards.
    pub async fn changed(&mut self) -> Result<(), Error> {
        if self.rx.recv().await.is_none() {
            return Err(Error::Monitor("monitor thread exited".into()));
        }
        tokio::time::sleep(SETTLE).await;
        while self.rx.try_recv().is_ok() {}
        Ok(())
    }
}

/// Without udev: list the ports every second and signal when the set changes.
#[cfg(not(target_os = "linux"))]
fn monitor_thread(
    tx: tokio::sync::mpsc::UnboundedSender<()>,
    ready: std::sync::mpsc::Sender<Result<(), String>>,
) {
    let names = || -> Option<Vec<String>> {
        let mut v: Vec<String> = serialport::available_ports()
            .ok()?
            .into_iter()
            .map(|p| p.port_name)
            .collect();
        v.sort();
        Some(v)
    };
    let Some(mut last) = names() else {
        let _ = ready.send(Err("cannot list serial ports".into()));
        return;
    };
    let _ = ready.send(Ok(()));
    while !tx.is_closed() {
        std::thread::sleep(std::time::Duration::from_secs(1));
        if let Some(now) = names()
            && now != last
        {
            last = now;
            if tx.send(()).is_err() {
                return;
            }
        }
    }
}

#[cfg(target_os = "linux")]
fn monitor_thread(
    tx: tokio::sync::mpsc::UnboundedSender<()>,
    ready: std::sync::mpsc::Sender<Result<(), String>>,
) {
    use std::os::fd::AsRawFd;

    let socket = match udev::MonitorBuilder::new()
        .and_then(|b| b.match_subsystem("tty"))
        // USB devices too, so a serial chip that gets no port is noticed (`advice`).
        .and_then(|b| b.match_subsystem_devtype("usb", "usb_device"))
        .and_then(|b| b.listen())
    {
        Ok(s) => s,
        Err(e) => {
            let _ = ready.send(Err(e.to_string()));
            return;
        }
    };
    let _ = ready.send(Ok(()));

    let mut pfd = libc::pollfd {
        fd: socket.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    while !tx.is_closed() {
        pfd.revents = 0;
        // SAFETY: poll on one valid pollfd that lives on this stack frame.
        let n = unsafe { libc::poll(&mut pfd, 1, POLL_MS) };
        if n < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return;
        }
        if n == 0 {
            continue;
        }
        // The socket is non-blocking; read every queued event.
        let relevant = socket
            .iter()
            .filter(|e| {
                matches!(
                    e.event_type(),
                    udev::EventType::Add | udev::EventType::Remove
                )
            })
            .count()
            > 0;
        if relevant && tx.send(()).is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    fn monitor_threads() -> usize {
        std::fs::read_dir("/proc/self/task")
            .unwrap()
            .filter_map(|t| std::fs::read_to_string(t.ok()?.path().join("comm")).ok())
            .filter(|name| name.trim() == "port-monitor")
            .count()
    }

    // Counts the monitor's thread by name in /proc.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn monitor_waits_quietly_and_its_thread_ends_on_drop() {
        // Plugging a board in is a manual check; here, nothing changes.
        let mut m = Monitor::new().unwrap();
        assert_eq!(monitor_threads(), 1);
        let r = tokio::time::timeout(std::time::Duration::from_millis(200), m.changed()).await;
        assert!(r.is_err(), "reported a change with nothing plugged in");

        drop(m);
        for _ in 0..30 {
            if monitor_threads() == 0 {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        panic!("udev monitor thread still running after drop");
    }

    #[test]
    fn classifies_common_bridges_without_guessing_the_chip() {
        assert_eq!(classify(0x10c4, 0xea60), (Some(Bridge::Cp210x), vec![]));
        assert_eq!(classify(0x1a86, 0x7523), (Some(Bridge::Ch340), vec![]));
        assert_eq!(classify(0x1a86, 0x55d4), (Some(Bridge::Ch9102), vec![]));
        assert_eq!(classify(0x0403, 0x6001), (Some(Bridge::Ftdi), vec![]));
        assert_eq!(classify(0x067b, 0x2303), (Some(Bridge::Pl2303), vec![]));
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

    fn usb_port(path: &str, vid: u16, pid: u16, access: Access) -> Port {
        let mut p = from_info(
            path.into(),
            serialport::SerialPortType::UsbPort(serialport::UsbPortInfo {
                vid,
                pid,
                serial_number: None,
                manufacturer: None,
                product: None,
            }),
        );
        p.access = access;
        p
    }

    #[test]
    fn remembers_the_chip_behind_an_adapter() {
        let t = tempfile::tempdir().unwrap();
        let known = Known::new(t.path().join("data/boards.json"));
        let mut with_serial = usb_port("/dev/ttyUSB0", 0x10c4, 0xea60, Access::Ok);
        with_serial.usb.as_mut().unwrap().serial = Some("0001".into());
        let without = usb_port("/dev/ttyUSB1", 0x1a86, 0x7523, Access::Ok);
        assert!(known.remember(&with_serial, "esp32").unwrap());
        assert!(!known.remember(&without, "esp32").unwrap());
        assert!(!known.remember(&with_serial, "esp32p4").unwrap());

        let mut ports = vec![with_serial.clone(), without];
        known.apply(&mut ports);
        assert_eq!(ports[0].chips, ["esp32"]);
        assert!(ports[0].identified);
        assert!(ports[1].chips.is_empty());
        assert!(!ports[1].identified);
        let v = serde_json::to_value(&ports[0]).unwrap();
        assert_eq!(v["identified"], true);
        assert!(
            serde_json::to_value(&ports[1])
                .unwrap()
                .get("identified")
                .is_none()
        );

        // An adapter nothing was found behind keeps the guess from its USB IDs.
        let mut jtag = vec![usb_port("/dev/ttyACM0", 0x303a, 0x1001, Access::Ok)];
        jtag[0].usb.as_mut().unwrap().serial = Some("0001".into());
        known.apply(&mut jtag);
        assert!(!jtag[0].identified);
    }

    #[test]
    fn advises_on_missing_and_unusable_boards() {
        // Nothing plugged in: a data cable, and on macOS and Windows a driver.
        assert_eq!(advice_from(&[], &[], "linux"), [Advice::DataCable]);
        assert_eq!(
            advice_from(&[], &[], "windows"),
            [Advice::DataCable, Advice::InstallDriver]
        );
        // A CH340 with no port (brltty), next to a mouse and a firmware's own device.
        assert_eq!(
            advice_from(
                &[],
                &[(0x1a86, 0x7523), (0x046d, 0xc52b), (0x303a, 0x0002)],
                "linux"
            ),
            [Advice::BridgeWithoutPort {
                bridge: Bridge::Ch340
            }]
        );
        // A port, usable: nothing to say; denied: access.
        let ok = usb_port("/dev/ttyUSB0", 0x10c4, 0xea60, Access::Ok);
        assert!(advice_from(std::slice::from_ref(&ok), &[(0x10c4, 0xea60)], "linux").is_empty());
        let denied = usb_port("/dev/ttyUSB0", 0x10c4, 0xea60, Access::Denied);
        assert_eq!(
            advice_from(&[denied], &[(0x10c4, 0xea60)], "linux"),
            [Advice::SerialAccess]
        );
        let v = serde_json::to_value(Advice::BridgeWithoutPort {
            bridge: Bridge::Cp210x,
        })
        .unwrap();
        assert_eq!(
            v,
            serde_json::json!({ "kind": "bridge-without-port", "bridge": "cp210x" })
        );
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
