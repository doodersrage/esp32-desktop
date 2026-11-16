//! What the app needs installed, whether it is, and installing what it can.
//!
//! Status checks never touch the network. Download sizes come from files already on
//! disk (the Arduino package index) or from pins (QEMU). arduino-cli itself is a
//! system package: the app shows the command to install it but never runs it,
//! because that needs root (CLAUDE.md rule 5).

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use esp32bench_build::{Stream, Toolchain};
use esp32bench_sim::{Arch, install, pin};
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
}

impl Id {
    pub const ALL: [Id; 4] = [
        Id::ArduinoCli,
        Id::Esp32Core,
        Id::QemuXtensa,
        Id::QemuRiscv32,
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
}

impl Env {
    /// `$ARDUINO_DIRECTORIES_DATA`, else `~/.arduino15`.
    pub fn default_arduino_data() -> Option<PathBuf> {
        if let Some(d) = std::env::var_os("ARDUINO_DIRECTORIES_DATA").filter(|d| !d.is_empty()) {
            return Some(PathBuf::from(d));
        }
        std::env::var_os("HOME")
            .filter(|h| !h.is_empty())
            .map(|h| PathBuf::from(h).join(".arduino15"))
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
    let (state, version, hint) = match probe {
        None => (
            State::Missing,
            None,
            Some("Install it from the Arch repositories: sudo pacman -S arduino-cli".to_owned()),
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
        download_bytes: None,
        installable: false,
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
    out
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
        Id::ArduinoCli => Err(InstallError::NotInstallable(id)),
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
