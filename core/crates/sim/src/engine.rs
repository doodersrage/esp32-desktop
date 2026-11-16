//! Which QEMU a simulation runs on, and how to start it.
//!
//! lcgamboa's fork is the simulator (docs/PLAN.md). ESP32 and ESP32-C3 run through
//! `esp32bench-qemu-host` with the fork's library build, so GPIO and I2C behave like
//! a board with nothing attached (the fork's plain executables ACK every I2C address).
//! The ESP32-S3 has no PICSimLab machine and runs on the fork's executable. Until the
//! fork is built, simulations run on Espressif's prebuilt QEMU instead.

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
    /// lcgamboa's fork (through the host process for ESP32 and C3).
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
    /// with the fork's library, so not on the S3 or Espressif's QEMU.
    pub hooks: bool,
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
    let dir = exe.parent()?;
    [Some(dir), dir.parent()]
        .into_iter()
        .flatten()
        .map(|d| d.join(HOST_BIN))
        .find(|p| p.is_file())
}

/// Launch settings on the fork's build `build`, with the host at `host` when present.
/// None when the chip needs the host and there is none.
pub fn lcgamboa_launch(chip: Chip, build: &Build, host: Option<&Path>) -> Option<Launch> {
    let args = build.qemu_args();
    let machine = Some(lcgamboa::machine(chip).to_owned());
    if chip == Chip::Esp32s3 {
        return Some(Launch {
            engine: Engine::Lcgamboa,
            qemu: build.executable(chip.arch()),
            machine,
            args,
            env: Vec::new(),
            hooks: false,
        });
    }
    let host = host?;
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
            (Chip::Esp32, "libqemu-xtensa.so", "esp32-picsimlab"),
            (Chip::Esp32c3, "libqemu-riscv32.so", "esp32c3-picsimlab"),
        ] {
            let l = lcgamboa_launch(chip, &build(), Some(Path::new("/bin/host"))).unwrap();
            assert_eq!(l.engine, Engine::Lcgamboa);
            assert_eq!(l.qemu, Path::new("/bin/host"));
            assert_eq!(l.machine.as_deref(), Some(machine));
            assert_eq!(l.args, ["-L", "/q/src/pc-bios"]);
            assert_eq!(l.env[0].0, "ESP32BENCH_QEMU_LIB");
            assert!(Path::new(&l.env[0].1).ends_with(lib));
            // No socket or pins: nothing is attached.
            assert_eq!(l.env.len(), 1);
            assert!(l.hooks);
        }
        assert_eq!(lcgamboa_launch(Chip::Esp32, &build(), None), None);
    }

    #[test]
    fn s3_runs_on_the_fork_executable() {
        let l = lcgamboa_launch(Chip::Esp32s3, &build(), None).unwrap();
        assert!(l.qemu.ends_with("src/build-exe/qemu-system-xtensa"));
        assert_eq!(l.machine.as_deref(), Some("esp32s3"));
        assert!(l.env.is_empty());
        assert!(!l.hooks, "the fork has no GPIO hooks on the S3");
    }

    #[test]
    fn engine_names_round_trip() {
        for e in [Engine::Lcgamboa, Engine::Espressif] {
            assert_eq!(e.as_str().parse::<Engine>().unwrap(), e);
            assert_eq!(serde_json::to_value(e).unwrap(), e.as_str());
        }
    }
}
