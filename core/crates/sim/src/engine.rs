//! Which QEMU a simulation runs on, and how to start it.
//!
//! lcgamboa's fork is the simulator (docs/PLAN.md). The ESP32, ESP32-S3 and ESP32-C3
//! run through `esp32bench-qemu-host` with the fork's library build, so GPIO and I2C
//! behave like a board with nothing attached (the fork's plain executables ACK every
//! I2C address). The S3 has no PICSimLab machine of its own: its `esp32s3` machine
//! takes the same hooks (patch 0015). Until the fork is built, simulations run on
//! Espressif's prebuilt QEMU instead.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::lcgamboa::{self, Build};
use crate::{Chip, Error, SessionConfig, install};

pub const HOST_BIN: &str = "esp32bench-qemu-host";
pub const HOST_ENV: &str = "ESP32BENCH_QEMU_HOST";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Engine {
    /// lcgamboa's fork (through the host process and its library).
    Lcgamboa,
    /// Espressif's prebuilt QEMU, used until the fork is built.
    Espressif,
}

impl Engine {
    pub fn as_str(self) -> &'static str {
        match self {
            Engine::Lcgamboa => "lcgamboa",
            Engine::Espressif => "espressif",
        }
    }
}

impl std::str::FromStr for Engine {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "lcgamboa" => Ok(Engine::Lcgamboa),
            "espressif" => Ok(Engine::Espressif),
            _ => Err(format!("unknown engine `{s}` (lcgamboa or espressif)")),
        }
    }
}

/// How to start QEMU for one chip: the program, machine, arguments and environment
/// to put in a [`SessionConfig`].
#[derive(Debug, Clone, PartialEq)]
pub struct Launch {
    pub engine: Engine,
    pub qemu: PathBuf,
    pub machine: Option<String>,
    pub args: Vec<OsString>,
    pub env: Vec<(OsString, OsString)>,
    /// Whether GPIOs can be bridged ([`SessionConfig::bridge`]): only through the host
    /// with the fork's library, so not on Espressif's QEMU.
    pub hooks: bool,
    /// An eFuse image the machine needs ([`SessionConfig::efuse`]).
    pub efuse: Option<Vec<u8>>,
    /// Whether this engine can wire the C3's USB serial port
    /// ([`SessionConfig::usb_console`]). Sessions turn it on only for boards that need it:
    /// with it on, an IDF boot's UART0 output sometimes stopped (about one in five).
    pub usb_console: bool,
    /// Whether the machine has a Wi-Fi radio for the simulated network
    /// ([`SessionConfig::network`]): the fork's ESP32, S3 and C3.
    pub network: bool,
}

impl Launch {
    /// A session config for `image` with these settings.
    pub fn config(&self, chip: Chip, image: PathBuf) -> SessionConfig {
        SessionConfig {
            chip,
            qemu: self.qemu.clone(),
            image,
            gdb_port: None,
            start_paused: false,
            machine: self.machine.clone(),
            extra_args: self.args.clone(),
            env: self.env.clone(),
            bridge: Vec::new(),
            circuit: None,
            efuse: self.efuse.clone(),
            usb_console: false,
            network: None,
            icount: false,
        }
    }
}

/// The host program: `ESP32BENCH_QEMU_HOST`, else next to the running executable (or
/// one level up, for test binaries in `target/*/deps/`).
pub fn find_host() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os(HOST_ENV).filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(p)).filter(|p| p.is_file());
    }
    let exe = std::env::current_exe().ok()?;
    host_candidates(exe.parent()?)
        .into_iter()
        .find(|p| p.is_file())
}

/// Where the host may be, for an executable in `dir`: beside it (dev builds), one
/// level up (test binaries in `target/*/deps/`), and in `../lib/esp32bench/` (the
/// package: /usr/bin/esp32benchd and /usr/lib/esp32bench/esp32bench-qemu-host, since
/// it is a helper, not a command).
fn host_candidates(dir: &Path) -> Vec<PathBuf> {
    let mut v = vec![dir.join(HOST_BIN)];
    if let Some(up) = dir.parent() {
        v.push(up.join(HOST_BIN));
        v.push(up.join("lib/esp32bench").join(HOST_BIN));
    }
    v
}

/// Launch settings on the fork's build `build`, with the host at `host` when present.
/// None when the chip needs the host and there is none.
pub fn lcgamboa_launch(chip: Chip, build: &Build, host: Option<&Path>) -> Option<Launch> {
    let args = build.qemu_args();
    let machine = Some(lcgamboa::machine(chip).to_owned());
    let host = host?;
    let mut args = args;
    // A Wi-Fi radio on hub 0. Alone there, its frames go nowhere; a session that asks
    // for the simulated network ([`crate::network`]) puts the restricted user-mode
    // network on the same hub. Without a radio the fork's Wi-Fi
    // start-up crashes (esp_phy_enable). The C3's radio reads its MAC address from the
    // eFuses before the eFuse device is ready, and aborts unless they come from a file:
    // `efuse` below. The S3's radio is the C3's model (patch 0012).
    let model = if chip == Chip::Esp32 {
        "esp32_wifi"
    } else {
        "esp32c3_wifi"
    };
    args.extend([
        "-nic".into(),
        format!("hubport,hubid=0,model={model}").into(),
    ]);
    Some(Launch {
        engine: Engine::Lcgamboa,
        qemu: host.to_owned(),
        machine,
        args,
        env: vec![(
            "ESP32BENCH_QEMU_LIB".into(),
            build.library(chip.arch()).into(),
        )],
        hooks: true,
        efuse: match chip {
            Chip::Esp32c3 => Some(lcgamboa::esp32c3_efuse()),
            // The ADC reference voltage, for ADC calibration (analogReadMilliVolts).
            Chip::Esp32 => Some(lcgamboa::esp32_efuse()),
            // ADC calibration's version (analogReadMilliVolts).
            Chip::Esp32s3 => Some(lcgamboa::esp32s3_efuse()),
        },
        usb_console: chip == Chip::Esp32c3,
        network: true,
    })
}

/// How to run `chip`: on `want` if given, else the fork when it is built (and the
/// host found), else Espressif's QEMU.
pub fn launch(chip: Chip, want: Option<Engine>) -> Result<Launch, Error> {
    let fork = || {
        let build = Build::find()?;
        lcgamboa_launch(chip, &build, find_host().as_deref())
    };
    let espressif = || -> Result<Launch, Error> {
        Ok(Launch {
            engine: Engine::Espressif,
            qemu: install::find_qemu(chip.arch())?,
            machine: None,
            args: Vec::new(),
            env: Vec::new(),
            hooks: false,
            efuse: None,
            usb_console: false,
            network: false,
        })
    };
    match want {
        Some(Engine::Lcgamboa) => fork().ok_or(Error::EngineMissing(
            "lcgamboa's QEMU is not built (or esp32bench-qemu-host is missing); \
             run `esp32bench toolchain install qemu-lcgamboa`",
        )),
        Some(Engine::Espressif) => espressif(),
        None => match fork() {
            Some(l) => Ok(l),
            None => espressif(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build() -> Build {
        Build { root: "/q".into() }
    }

    #[test]
    fn esp32_and_c3_go_through_the_host_with_the_library() {
        for (chip, lib, machine) in [
            (Chip::Esp32, "xtensa", "esp32-picsimlab"),
            (Chip::Esp32c3, "riscv32", "esp32c3-picsimlab"),
        ] {
            let l = lcgamboa_launch(chip, &build(), Some(Path::new("/bin/host"))).unwrap();
            assert_eq!(l.engine, Engine::Lcgamboa);
            assert_eq!(l.qemu, Path::new("/bin/host"));
            assert_eq!(l.machine.as_deref(), Some(machine));
            assert_eq!(&l.args[..2], ["-L", "/q/src/pc-bios"]);
            // A radio on an empty hub: no network (rule 4).
            let nic = if chip == Chip::Esp32 {
                "esp32_wifi"
            } else {
                "esp32c3_wifi"
            };
            assert_eq!(
                &l.args[2..],
                ["-nic", &format!("hubport,hubid=0,model={nic}")]
            );
            // The C3's USB serial port can be wired (patch 0006), but only on request.
            assert_eq!(l.usb_console, chip == Chip::Esp32c3);
            assert!(!l.config(chip, "x".into()).usb_console);
            // The C3's radio needs an eFuse image: chip revision v0.3 and a MAC.
            // The ESP32's carries the ADC reference voltage: 1100 mV.
            let e = l.efuse.as_deref().unwrap();
            assert_eq!(e.len(), 1024);
            if chip == Chip::Esp32c3 {
                assert_eq!(e[24..30], [0x01, 0x10, 0xfe, 0x24, 0x0a, 0xc4]);
            } else {
                assert_eq!(
                    u32::from_le_bytes(e[16..20].try_into().unwrap()) >> 8 & 0x1f,
                    0x10
                );
                assert!(e[4..12].iter().all(|b| *b == 0), "the fork's default MAC");
            }
            assert_eq!(l.env[0].0, "ESP32BENCH_QEMU_LIB");
            assert!(
                Path::new(&l.env[0].1)
                    .ends_with(format!("libqemu-{lib}.{}", lcgamboa::LIBRARY_EXT))
            );
            // No socket or pins: nothing is attached.
            assert_eq!(l.env.len(), 1);
            assert!(l.hooks);
        }
        assert_eq!(lcgamboa_launch(Chip::Esp32, &build(), None), None);
    }

    #[test]
    fn host_is_found_beside_the_daemon_or_in_the_packages_lib_dir() {
        assert_eq!(
            host_candidates(Path::new("/usr/bin")),
            [
                Path::new("/usr/bin/esp32bench-qemu-host"),
                Path::new("/usr/esp32bench-qemu-host"),
                Path::new("/usr/lib/esp32bench/esp32bench-qemu-host"),
            ]
        );
    }

    #[test]
    fn s3_runs_through_the_host_with_hooks() {
        let host = Path::new("/usr/lib/esp32bench/esp32bench-qemu-host");
        let l = lcgamboa_launch(Chip::Esp32s3, &build(), Some(host)).unwrap();
        assert_eq!(l.qemu, host);
        assert_eq!(l.machine.as_deref(), Some("esp32s3"));
        assert!(l.hooks, "patch 0015 gives the S3 the library's hooks");
        assert!(
            Path::new(&l.env[0].1).ends_with(format!("libqemu-xtensa.{}", lcgamboa::LIBRARY_EXT))
        );
        assert_eq!(
            l.efuse.as_deref().map(|e| e[64]),
            Some(1),
            "ADC calibration V1"
        );
        assert!(!l.usb_console);
        // The C3's radio model, on hub 0 (patch 0012); the network is the session's.
        assert_eq!(&l.args[2..], ["-nic", "hubport,hubid=0,model=esp32c3_wifi"]);
        assert!(l.network);
        assert_eq!(lcgamboa_launch(Chip::Esp32s3, &build(), None), None);
    }

    #[test]
    fn engine_names_round_trip() {
        for e in [Engine::Lcgamboa, Engine::Espressif] {
            assert_eq!(e.as_str().parse::<Engine>().unwrap(), e);
            assert_eq!(serde_json::to_value(e).unwrap(), e.as_str());
        }
    }
}
