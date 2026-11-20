//! What the app needs installed, whether it is, and installing what it can.
//!
//! Status checks never touch the network. Download sizes come from files already on
//! disk (the Arduino package index) or from pins (QEMU, arduino-cli). On Arch
//! arduino-cli is a system package: the app shows the command to install it but never
//! runs it, because that needs root (CLAUDE.md rule 5). Debian, Ubuntu and Fedora have
//! no current package for it, so there the app installs a pinned official release
//! into its own data directory, which needs no root.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use esp32bench_build::{Stream, Toolchain};
use esp32bench_distro::{Distro, Need};
use esp32bench_sim::{Arch, install, lcgamboa, pin};
use serde::Serialize;
use serde_json::Value;
use tokio::process::Command;

/// The ESP32 Arduino core the app builds with. Tests pin the same version in
/// tests/firmware/ARDUINO_ESP32_VERSION; a test keeps the two equal.
pub const ARDUINO_ESP32_VERSION: &str = "3.3.12";
pub const ARDUINO_ESP32_INDEX_URL: &str =
    "https://espressif.github.io/arduino-esp32/package_esp32_index.json";
const PROBE_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Id {
    ArduinoCli,
    Esp32Core,
    QemuXtensa,
    QemuRiscv32,
    QemuLcgamboa,
    DeviceAccess,
}

impl Id {
    pub const ALL: [Id; 6] = [
        Id::ArduinoCli,
        Id::Esp32Core,
        Id::QemuXtensa,
        Id::QemuRiscv32,
        Id::QemuLcgamboa,
        Id::DeviceAccess,
    ];

    pub fn parse(s: &str) -> Option<Id> {
        Id::ALL.into_iter().find(|id| id.as_str() == s)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Id::ArduinoCli => "arduino-cli",
            Id::Esp32Core => "esp32-core",
            Id::QemuXtensa => "qemu-xtensa",
            Id::QemuRiscv32 => "qemu-riscv32",
            Id::QemuLcgamboa => "qemu-lcgamboa",
            Id::DeviceAccess => "device-access",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum State {
    Installed,
    Missing,
    /// Installed, but not the version this program is pinned to.
    OtherVersion,
    /// Present, but it fails when run; `hint` has its error.
    Broken,
}

/// One tool, as `toolchain.status` reports it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Component {
    pub id: Id,
    pub name: &'static str,
    /// What it is needed for: "build" or "simulator".
    pub needed_for: &'static str,
    pub state: State,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// The version this program wants, when it pins one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wanted: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    /// Bytes to download to install it, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub download_bytes: Option<u64>,
    /// Whether `toolchain.install` can install it.
    pub installable: bool,
    /// What to do by hand, when the app cannot install it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

/// Where to look. Tests point these at stand-ins.
#[derive(Debug, Clone)]
pub struct Env {
    /// arduino-cli to use; None means look on PATH.
    pub arduino_cli: Option<PathBuf>,
    /// The Arduino data directory (default ~/.arduino15), for package index files.
    pub arduino_data: PathBuf,
    /// Private sketchbook for builds (ARDUINO_DIRECTORIES_USER).
    pub sketchbook: PathBuf,
    /// The distribution, for install hints and what the app installs itself
    /// (`Distro::detect()`; tests set it).
    pub distro: Distro,
}

/// The udev rule file the package installs (packaging/udev/).
pub const UDEV_RULE: &str = "69-esp32bench.rules";
/// Where udev looks for it.
pub const UDEV_DIRS: [&str; 2] = ["/usr/lib/udev/rules.d", "/etc/udev/rules.d"];

/// Whether this user can reach boards without root: the esp32bench udev rule is
/// installed (seat access), or the user is in the group that owns serial ports
/// (`uucp` on Arch, `dialout` on Debian, Ubuntu and Fedora).
pub fn board_access(rule_installed: bool, in_serial_group: bool, distro: Distro) -> Component {
    let ok = rule_installed || in_serial_group;
    let group = distro.serial_group();
    Component {
        id: Id::DeviceAccess,
        name: "Board access",
        needed_for: "hardware",
        state: if ok { State::Installed } else { State::Missing },
        version: None,
        wanted: None,
        path: None,
        download_bytes: None,
        installable: false,
        hint: (!ok).then(|| {
            format!(
                "Install the esp32bench package, which ships a udev rule, or join the {group} \
                 group: sudo usermod -aG {group} $USER, then log out and in again."
            )
        }),
    }
}

/// Board access where there is no udev: on macOS the user may open USB serial ports
/// as they are; Windows needs the USB-UART bridge's driver (Espressif's own USB comes
/// with the system).
pub fn board_access_without_udev() -> Component {
    Component {
        id: Id::DeviceAccess,
        name: "Board access",
        needed_for: "hardware",
        state: State::Installed,
        version: None,
        wanted: None,
        path: None,
        download_bytes: None,
        installable: false,
        hint: cfg!(windows).then(|| {
            "Boards with a CP210x or CH340 USB chip need its driver from the chip's maker \
             (Silicon Labs, WCH) before they show up."
                .to_owned()
        }),
    }
}

/// Whether the current user is a member of `group` (primary or supplementary).
#[cfg(unix)]
pub fn in_group(group: &str) -> bool {
    let Ok(name) = std::ffi::CString::new(group) else {
        return false;
    };
    // SAFETY: getgrnam returns NULL or a pointer to static storage, read at once.
    let gid = unsafe {
        let g = libc::getgrnam(name.as_ptr());
        if g.is_null() {
            return false;
        }
        (*g).gr_gid
    };
    // SAFETY: getegid has no preconditions.
    if unsafe { libc::getegid() } == gid {
        return true;
    }
    // SAFETY: getgroups(0, NULL) returns the count; then fill a buffer that size.
    let n = unsafe { libc::getgroups(0, std::ptr::null_mut()) };
    if n <= 0 {
        return false;
    }
    let mut groups = vec![0 as libc::gid_t; n as usize];
    // SAFETY: the buffer holds `n` entries.
    let n = unsafe { libc::getgroups(n, groups.as_mut_ptr()) };
    n > 0 && groups[..n as usize].contains(&gid)
}

impl Env {
    /// `$ARDUINO_DIRECTORIES_DATA`, else arduino-cli's default for this system
    /// (`~/.arduino15` on Linux; `esp32bench_distro::dirs::arduino_data`).
    pub fn default_arduino_data() -> Option<PathBuf> {
        esp32bench_distro::dirs::arduino_data()
    }

    fn toolchain(&self) -> Option<Toolchain> {
        match &self.arduino_cli {
            Some(p) => Some(Toolchain {
                arduino_cli: p.clone(),
                sketchbook: self.sketchbook.clone(),
            }),
            None => Toolchain::detect(self.sketchbook.clone()).ok(),
        }
    }
}

/// Run arduino-cli with `args` and parse its JSON output, or say why it failed.
async fn cli_json(cli: &Path, args: &[&str]) -> Result<Value, String> {
    let out = tokio::time::timeout(
        PROBE_TIMEOUT,
        Command::new(cli).args(args).kill_on_drop(true).output(),
    )
    .await
    .map_err(|_| "it did not answer within 20 seconds".to_owned())?
    .map_err(|e| e.to_string())?;
    let json: Option<Value> = serde_json::from_slice(&out.stdout).ok();
    if !out.status.success() {
        // Asked for JSON, arduino-cli reports errors as {"error": ...}; 1.5 writes that
        // to stderr.
        let err_json: Option<Value> = serde_json::from_slice(&out.stderr).ok();
        let msg = [json.as_ref(), err_json.as_ref()]
            .into_iter()
            .flatten()
            .find_map(|v| v["error"].as_str().map(str::to_owned))
            .unwrap_or_else(|| String::from_utf8_lossy(&out.stderr).trim().to_owned());
        return Err(if msg.is_empty() {
            out.status.to_string()
        } else {
            msg
        });
    }
    json.ok_or_else(|| "it printed something other than JSON".to_owned())
}

/// Check every component.
pub async fn status(env: &Env) -> Vec<Component> {
    // arduino-cli: missing, present but failing, or working.
    let tools = env.toolchain().filter(|t| t.arduino_cli.is_file());
    let mut out = Vec::new();
    let probe = match &tools {
        Some(t) => Some(
            cli_json(&t.arduino_cli, &["version", "--format", "json"])
                .await
                .and_then(|v| {
                    v["VersionString"]
                        .as_str()
                        .map(str::to_owned)
                        .ok_or_else(|| "it did not report a version".to_owned())
                }),
        ),
        None => None,
    };
    let cli_ok = matches!(probe, Some(Ok(_)));
    let distro = env.distro;
    // Where the distribution packages it, that; else the app installs its own copy.
    let own_cli = distro.package(Need::ArduinoCli).is_none();
    let (state, version, hint) = match probe {
        None => (
            State::Missing,
            None,
            Some(match distro.install_hint(&[Need::ArduinoCli]) {
                Some(cmd) => format!("Install it from the {} repositories: {cmd}", distro.name()),
                None => format!(
                    "{} has no current package for it, so the app installs arduino-cli {} \
                     (an official release, checked) into its own folder.",
                    distro.name(),
                    esp32bench_build::arduino_cli_pin::VERSION
                ),
            }),
        ),
        Some(Ok(v)) => (State::Installed, Some(v), None),
        Some(Err(e)) => (
            State::Broken,
            None,
            Some(format!("arduino-cli is installed but fails: {e}")),
        ),
    };
    out.push(Component {
        id: Id::ArduinoCli,
        name: "arduino-cli",
        needed_for: "build",
        state,
        version,
        wanted: None,
        path: tools.as_ref().map(|t| t.arduino_cli.clone()),
        download_bytes: (own_cli && state == State::Missing)
            .then_some(esp32bench_build::arduino_cli_pin::SIZE),
        installable: own_cli && state != State::Installed,
        hint,
    });

    // The ESP32 Arduino core
    let core_version = match (&tools, cli_ok) {
        (Some(t), true) => cli_json(&t.arduino_cli, &["core", "list", "--format", "json"])
            .await
            .ok()
            .and_then(|v| installed_core_version(&v)),
        _ => None,
    };
    let core_state = match &core_version {
        None => State::Missing,
        Some(v) if v == ARDUINO_ESP32_VERSION => State::Installed,
        Some(_) => State::OtherVersion,
    };
    out.push(Component {
        id: Id::Esp32Core,
        name: "ESP32 Arduino core",
        needed_for: "build",
        state: core_state,
        version: core_version,
        wanted: Some(ARDUINO_ESP32_VERSION.to_owned()),
        path: None,
        download_bytes: if core_state == State::Installed {
            None
        } else {
            core_download_bytes(&env.arduino_data, ARDUINO_ESP32_VERSION)
        },
        installable: cli_ok,
        hint: (!cli_ok).then(|| "Needs a working arduino-cli first.".to_owned()),
    });

    // QEMU, one per architecture
    for (id, arch, name) in [
        (Id::QemuXtensa, Arch::Xtensa, "QEMU for ESP32 and ESP32-S3"),
        (Id::QemuRiscv32, Arch::Riscv32, "QEMU for ESP32-C3"),
    ] {
        let found = install::find_qemu(arch).ok();
        out.push(Component {
            id,
            name,
            needed_for: "simulator",
            state: if found.is_some() {
                State::Installed
            } else {
                State::Missing
            },
            version: found.as_ref().map(|_| pin::QEMU_RELEASE.to_owned()),
            wanted: Some(pin::QEMU_RELEASE.to_owned()),
            download_bytes: found.is_none().then(|| pin::asset(arch).size),
            path: found,
            installable: true,
            hint: None,
        });
    }

    let outdated = lcgamboa::root().is_ok_and(|r| lcgamboa::Build::outdated_at(&r));
    out.push(lcgamboa_status(
        lcgamboa::Build::find(),
        outdated,
        &lcgamboa::missing_requirements().await,
        env.distro,
    ));

    #[cfg(target_os = "linux")]
    {
        let rule = UDEV_DIRS
            .iter()
            .any(|d| Path::new(d).join(UDEV_RULE).is_file());
        out.push(board_access(
            rule,
            in_group(env.distro.serial_group()),
            env.distro,
        ));
    }
    #[cfg(not(target_os = "linux"))]
    out.push(board_access_without_udev());
    out
}

/// lcgamboa's fork, built from source: what is missing to build it, if it is not built.
pub fn lcgamboa_status(
    build: Option<lcgamboa::Build>,
    outdated: bool,
    missing: &[lcgamboa::Requirement],
    distro: Distro,
) -> Component {
    // On macOS and Windows the package carries the simulator (lcgamboa::bundled_root);
    // it is only built from source on Linux.
    let shipped = !cfg!(target_os = "linux");
    let hint = match (&build, lcgamboa::install_hint(distro, missing)) {
        (Some(_), _) => None,
        (None, _) if shipped => Some(
            "The simulator comes with the app on this system, and it is not where it should be: \
             reinstall esp32bench."
                .to_owned(),
        ),
        (None, None) if outdated => {
            Some("An older build is installed; rebuild it (several minutes).".to_owned())
        }
        (None, Some(cmd)) => Some(format!(
            "Building it needs tools that are not installed: {cmd}"
        )),
        (None, None) => Some(
            "Built from source on this computer: several minutes and about 2 GB of disk."
                .to_owned(),
        ),
    };
    Component {
        id: Id::QemuLcgamboa,
        name: "QEMU with GPIO and I2C (lcgamboa)",
        needed_for: "simulator",
        state: match (&build, outdated) {
            (Some(_), _) => State::Installed,
            (None, true) => State::OtherVersion,
            (None, false) => State::Missing,
        },
        version: build.as_ref().map(|_| lcgamboa::short_commit().to_owned()),
        wanted: Some(lcgamboa::short_commit().to_owned()),
        path: build.map(|b| b.root),
        download_bytes: None,
        installable: !shipped && missing.is_empty(),
        hint,
    }
}

/// The installed version of esp32:esp32 from `arduino-cli core list --format json`.
pub fn installed_core_version(core_list: &Value) -> Option<String> {
    core_list["platforms"]
        .as_array()?
        .iter()
        .find(|p| p["id"] == "esp32:esp32")?
        .get("installed_version")?
        .as_str()
        .map(str::to_owned)
}

/// Bytes to download for esp32:esp32 at `version` and its tools for this host, from
/// the package index files arduino-cli keeps. None if the index is not there yet.
pub fn core_download_bytes(arduino_data: &Path, version: &str) -> Option<u64> {
    let read = |name: &str| -> Option<Value> {
        serde_json::from_slice(&std::fs::read(arduino_data.join(name)).ok()?).ok()
    };
    let esp32 = read("package_esp32_index.json")?;
    let arduino = read("package_index.json");
    let packages: Vec<&Value> = esp32["packages"]
        .as_array()?
        .iter()
        .chain(
            arduino
                .iter()
                .flat_map(|a| a["packages"].as_array().into_iter().flatten()),
        )
        .collect();
    let package = packages.iter().find(|p| p["name"] == "esp32")?;
    let platform = package["platforms"]
        .as_array()?
        .iter()
        .find(|p| p["version"] == version)?;
    let mut total = size_of(&platform["size"])?;
    for dep in platform["toolsDependencies"].as_array()? {
        let tool = packages
            .iter()
            .filter(|p| p["name"] == dep["packager"])
            .flat_map(|p| p["tools"].as_array().into_iter().flatten())
            .find(|t| t["name"] == dep["name"] && t["version"] == dep["version"]);
        let host_size = tool.and_then(|t| {
            t["systems"].as_array()?.iter().find_map(|s| {
                let host = s["host"].as_str()?;
                (host == "x86_64-pc-linux-gnu" || host == "x86_64-linux-gnu")
                    .then(|| size_of(&s["size"]))
                    .flatten()
            })
        });
        total += host_size.unwrap_or(0);
    }
    Some(total)
}

/// Index sizes are usually strings of digits, but some tools (the per-chip libs)
/// use plain numbers.
fn size_of(v: &Value) -> Option<u64> {
    v.as_str()
        .and_then(|s| s.parse().ok())
        .or_else(|| v.as_u64())
}

#[derive(Debug)]
pub enum InstallError {
    /// The component cannot be installed by this program (e.g. needs root).
    NotInstallable(Id),
    Failed(String),
}

impl std::fmt::Display for InstallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InstallError::NotInstallable(id) => {
                write!(f, "{} cannot be installed from here", id.as_str())
            }
            InstallError::Failed(e) => f.write_str(e),
        }
    }
}

impl std::error::Error for InstallError {}

/// Install `id`, reporting progress lines to `log`. Dropping the future cancels it.
pub async fn install(env: &Env, id: Id, mut log: impl FnMut(String)) -> Result<(), InstallError> {
    match id {
        // A udev rule or group change needs root.
        Id::DeviceAccess => Err(InstallError::NotInstallable(id)),
        // Arch packages arduino-cli (installing it needs root); elsewhere the app
        // installs its own pinned copy.
        Id::ArduinoCli if env.distro.package(Need::ArduinoCli).is_some() => {
            Err(InstallError::NotInstallable(id))
        }
        Id::ArduinoCli => {
            use esp32bench_build::arduino_cli_pin as pin;
            let dir = esp32bench_build::app_arduino_cli_dir()
                .ok_or_else(|| InstallError::Failed("no home directory".into()))?;
            log(format!(
                "Downloading arduino-cli {} ({} MB) and checking its SHA-256…",
                pin::VERSION,
                pin::SIZE / 1_000_000
            ));
            let dir =
                install::install_archive(&pin::url(), pin::SHA256, &dir, Path::new(pin::BINARY))
                    .await
                    .map_err(|e| InstallError::Failed(e.to_string()))?;
            log(format!("Installed {}", dir.join(pin::BINARY).display()));
            Ok(())
        }
        Id::Esp32Core => {
            let tools = env
                .toolchain()
                .ok_or_else(|| InstallError::Failed("arduino-cli is not installed".into()))?;
            let mut lines = |_: Stream, l: String| log(l);
            let url = ARDUINO_ESP32_INDEX_URL;
            let pkg = format!("esp32:esp32@{ARDUINO_ESP32_VERSION}");
            for (step, args) in [
                (
                    "updating the board index",
                    vec!["core", "update-index", "--additional-urls", url],
                ),
                (
                    "installing the ESP32 core",
                    vec!["core", "install", &pkg, "--additional-urls", url],
                ),
            ] {
                let args: Vec<OsString> = args.into_iter().map(OsString::from).collect();
                esp32bench_build::run_arduino_cli(&tools, step, args, &mut lines)
                    .await
                    .map_err(|e| InstallError::Failed(e.to_string()))?;
            }
            Ok(())
        }
        Id::QemuLcgamboa => {
            log(format!(
                "Building lcgamboa's QEMU {} from source. This takes several minutes.",
                lcgamboa::short_commit()
            ));
            lcgamboa::build(log)
                .await
                .map(|_| ())
                .map_err(|e| InstallError::Failed(e.to_string()))
        }
        Id::QemuXtensa | Id::QemuRiscv32 => {
            let arch = if id == Id::QemuXtensa {
                Arch::Xtensa
            } else {
                Arch::Riscv32
            };
            log(format!(
                "Downloading QEMU {} for {arch} ({} MB) and checking its SHA-256…",
                pin::QEMU_RELEASE,
                pin::asset(arch).size / 1_000_000
            ));
            let path = install::install_qemu(arch)
                .await
                .map_err(|e| InstallError::Failed(e.to_string()))?;
            log(format!("Installed {}", path.display()));
            Ok(())
        }
    }
}
