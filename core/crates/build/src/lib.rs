//! Arduino builds of library projects.
//!
//! [`plan`] decides everything a build will do before anything runs: the board's
//! FQBN, the libraries to download, the partition scheme and a hash of the code. The
//! plan is what the user is shown and agrees to ([`Consent`]), and what [`run`] then
//! executes with `arduino-cli`, using explicit argument lists only.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use esp32bench_library::{Firmware, Manifest};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

/// Overrides the arduino-cli binary (tests use a stand-in).
pub const ARDUINO_CLI_ENV: &str = "ESP32BENCH_ARDUINO_CLI";
const CONSENT_LIFETIME: Duration = Duration::from_secs(10 * 60);

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("no project `{0}` in the library")]
    NoProject(String),
    #[error("`{0}` ships as prebuilt firmware; there is nothing to build")]
    Prebuilt(String),
    #[error("`{project}` has no build for board `{board}`")]
    NoBuild { project: String, board: String },
    #[error("board `{0}` is not one this program can build for")]
    UnknownBoard(String),
    #[error("board `{board}` has no `{scheme}` partition scheme, which `{project}` needs")]
    NoPartitionScheme {
        project: String,
        board: String,
        scheme: String,
    },
    #[error("`{0}` is not a safe id")]
    BadId(String),
    #[error("arduino-cli was not found; install the arduino-cli package")]
    NoArduinoCli,
    #[error("{step} failed ({status})")]
    StepFailed { step: &'static str, status: String },
    #[error("the build finished but {0} is missing")]
    MissingOutput(PathBuf),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Arduino-ESP32 board ids for each build target, and the partition scheme that gives
/// a project needing `huge_app` (a 3 MB app) its room: `huge_app` itself, or on the
/// XIAO ESP32-S3, which has no `huge_app` entry, its 8 MB default (a 3.2 MB app).
/// Checked against core 3.3.12's boards.txt.
const BOARDS: &[(&str, &str, &str)] = &[
    ("esp32", "esp32:esp32:esp32", "huge_app"),
    ("esp32s3", "esp32:esp32:esp32s3", "huge_app"),
    ("esp32c3", "esp32:esp32:esp32c3", "huge_app"),
    ("esp32s2", "esp32:esp32:esp32s2", "huge_app"),
    ("esp32c6", "esp32:esp32:esp32c6", "huge_app"),
    ("xiao-esp32c3", "esp32:esp32:XIAO_ESP32C3", "huge_app"),
    ("xiao-esp32s3", "esp32:esp32:XIAO_ESP32S3", "default_8MB"),
    ("esp32-cam", "esp32:esp32:esp32cam", "huge_app"),
    (
        "esp32c3-supermini",
        "esp32:esp32:nologo_esp32c3_super_mini",
        "huge_app",
    ),
];

/// Everything a build will do. Shown to the user before it runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    pub project: String,
    pub board: String,
    /// The board whose firmware is built (a variant builds as its parent).
    pub target: String,
    pub fqbn: String,
    /// SHA-256 of the sketch, so consent is for exactly this code.
    pub code_sha256: String,
    /// Arduino library names that will be downloaded and installed.
    pub install: Vec<String>,
}

/// Ids from downloaded data become path components; allow only `[a-z0-9-]`.
/// Whether builds for `board` put Arduino's `Serial` on the chip's USB-Serial-JTAG
/// (USB CDC on boot) rather than UART0: boards with no USB-UART chip. The simulator
/// then needs the USB console (lcgamboa's fork, patch 0006).
pub fn serial_on_usb(board: &str) -> bool {
    board == "esp32c3-supermini"
}

pub fn safe_id(id: &str) -> Result<(), Error> {
    let ok = !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !id.starts_with('-');
    if ok {
        Ok(())
    } else {
        Err(Error::BadId(id.to_owned()))
    }
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// Work out a build, checking it can be done, without running anything.
/// Returns the plan and the sketch code it covers.
pub fn plan(manifest: &Manifest, project: &str, board: &str) -> Result<(Plan, String), Error> {
    // Ids come from downloaded data and become path components.
    safe_id(project)?;
    safe_id(board)?;
    let p = manifest
        .project(project)
        .ok_or_else(|| Error::NoProject(project.to_owned()))?;
    if p.firmware == Firmware::Prebuilt {
        return Err(Error::Prebuilt(project.to_owned()));
    }
    let build = p
        .builds
        .iter()
        .find(|b| b.board == board)
        .ok_or_else(|| Error::NoBuild {
            project: project.to_owned(),
            board: board.to_owned(),
        })?;
    let code = build.code.clone().ok_or_else(|| Error::NoBuild {
        project: project.to_owned(),
        board: board.to_owned(),
    })?;
    let target = manifest
        .boards
        .iter()
        .find(|b| b.id == board)
        .and_then(|b| b.build_as.clone())
        .unwrap_or_else(|| board.to_owned());
    let &(_, base, big_app) = BOARDS
        .iter()
        .find(|(id, _, _)| *id == target)
        .ok_or_else(|| Error::UnknownBoard(target.clone()))?;

    // DIO flash mode works on every board, and QEMU only boots DIO or DOUT images,
    // so one build serves both the board and the simulator.
    let mut fqbn = format!("{base}:FlashMode=dio");
    if let Some(scheme) = &p.partitions {
        if scheme != "huge_app" {
            return Err(Error::NoPartitionScheme {
                project: project.to_owned(),
                board: target,
                scheme: scheme.clone(),
            });
        }
        fqbn.push_str(",PartitionScheme=");
        fqbn.push_str(big_app);
    }

    let plan = Plan {
        project: project.to_owned(),
        board: board.to_owned(),
        target,
        fqbn,
        code_sha256: hex(&Sha256::digest(code.as_bytes())),
        install: p
            .install
            .iter()
            .map(|i| i.arduino_name().to_owned())
            .collect(),
    };
    Ok((plan, code))
}

/// Single-use permission to run one exact plan, issued after the user has seen it
/// (rule 4: nothing downloaded runs without consent). Tokens expire.
pub struct Consent<P = Plan> {
    grants: Mutex<HashMap<String, (P, Instant)>>,
}

impl<P> Default for Consent<P> {
    fn default() -> Self {
        Consent {
            grants: Mutex::new(HashMap::new()),
        }
    }
}

impl<P: Clone + PartialEq> Consent<P> {
    pub fn new() -> Self {
        Self::default()
    }

    /// A token the user can give back to run `plan`.
    pub fn issue(&self, plan: &P) -> Result<String, Error> {
        let token = random_token()?;
        let mut g = self.grants.lock().unwrap_or_else(|p| p.into_inner());
        g.retain(|_, (_, at)| at.elapsed() < CONSENT_LIFETIME);
        g.insert(token.clone(), (plan.clone(), Instant::now()));
        Ok(token)
    }

    /// True if `token` was issued for exactly `plan` and has not expired. The token is
    /// used up either way.
    pub fn redeem(&self, token: &str, plan: &P) -> bool {
        let mut g = self.grants.lock().unwrap_or_else(|p| p.into_inner());
        match g.remove(token) {
            Some((granted, at)) => &granted == plan && at.elapsed() < CONSENT_LIFETIME,
            None => false,
        }
    }

    #[doc(hidden)]
    pub fn age_for_test(&self, token: &str, by: Duration) {
        let mut g = self.grants.lock().unwrap_or_else(|p| p.into_inner());
        if let Some((_, at)) = g.get_mut(token)
            && let Some(earlier) = at.checked_sub(by)
        {
            *at = earlier;
        }
    }
}

fn random_token() -> Result<String, Error> {
    use std::io::Read;
    let mut buf = [0u8; 16];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut buf)?;
    Ok(hex(&buf))
}

/// `$XDG_DATA_HOME/esp32bench/arduino` (or `~/.local/share/...`): the sketchbook
/// whose `libraries/` folder holds what builds install.
pub fn default_sketchbook() -> Option<PathBuf> {
    let data = std::env::var_os("XDG_DATA_HOME")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .filter(|h| !h.is_empty())
                .map(|h| PathBuf::from(h).join(".local/share"))
        })?;
    Some(data.join("esp32bench").join("arduino"))
}

/// Where arduino-cli is, and the private sketchbook its libraries go into.
#[derive(Debug, Clone)]
pub struct Toolchain {
    pub arduino_cli: PathBuf,
    /// `ARDUINO_DIRECTORIES_USER`: keeps installed libraries out of ~/Arduino.
    pub sketchbook: PathBuf,
}

impl Toolchain {
    /// `$ESP32BENCH_ARDUINO_CLI`, else `arduino-cli` on PATH.
    pub fn detect(sketchbook: PathBuf) -> Result<Self, Error> {
        let cli = match std::env::var_os(ARDUINO_CLI_ENV).filter(|v| !v.is_empty()) {
            Some(p) => PathBuf::from(p),
            None => find_on_path("arduino-cli").ok_or(Error::NoArduinoCli)?,
        };
        Ok(Toolchain {
            arduino_cli: cli,
            sketchbook,
        })
    }
}

fn find_on_path(name: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|d| d.join(name))
        .find(|p| p.is_file())
}

/// The directories one build uses, under `root/<project>/<board>/`.
#[derive(Debug, Clone)]
pub struct Dirs {
    pub sketch: PathBuf,
    pub build: PathBuf,
    pub out: PathBuf,
}

impl Dirs {
    pub fn new(root: &Path, plan: &Plan) -> Self {
        let base = root.join(&plan.project).join(&plan.board);
        Dirs {
            // arduino-cli wants the folder named after the .ino file.
            sketch: base.join("sketch").join(&plan.project),
            build: base.join("build"),
            out: base.join("out"),
        }
    }

    pub fn ino(&self, plan: &Plan) -> PathBuf {
        self.sketch.join(format!("{}.ino", plan.project))
    }
}

pub fn lib_install_args(plan: &Plan) -> Vec<OsString> {
    let mut a: Vec<OsString> = vec!["lib".into(), "install".into()];
    a.extend(plan.install.iter().map(OsString::from));
    a
}

pub fn compile_args(plan: &Plan, dirs: &Dirs) -> Vec<OsString> {
    vec![
        "compile".into(),
        "--fqbn".into(),
        plan.fqbn.clone().into(),
        "--build-path".into(),
        dirs.build.clone().into(),
        "--output-dir".into(),
        dirs.out.clone().into(),
        dirs.sketch.clone().into(),
    ]
}

/// Which output stream a log line came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Stream {
    Stdout,
    Stderr,
}

/// What a successful build produced.
#[derive(Debug, Clone, Serialize)]
pub struct Output {
    /// Merged flash image (bootloader, partitions, app), for flashing or QEMU.
    pub image: PathBuf,
    pub elf: PathBuf,
}

/// Run a plan: write the sketch, install libraries, compile. Every line arduino-cli
/// prints goes to `log`. Dropping the returned future kills arduino-cli and anything
/// it started.
pub async fn run(
    plan: &Plan,
    code: &str,
    tools: &Toolchain,
    dirs: &Dirs,
    mut log: impl FnMut(Stream, String),
) -> Result<Output, Error> {
    tokio::fs::create_dir_all(&dirs.sketch).await?;
    tokio::fs::create_dir_all(&dirs.build).await?;
    tokio::fs::create_dir_all(&dirs.out).await?;
    tokio::fs::create_dir_all(&tools.sketchbook).await?;
    tokio::fs::write(dirs.ino(plan), code).await?;

    if !plan.install.is_empty() {
        step(
            tools,
            "installing libraries",
            lib_install_args(plan),
            &mut log,
        )
        .await?;
    }
    step(tools, "compiling", compile_args(plan, dirs), &mut log).await?;

    let out = Output {
        image: dirs.out.join(format!("{}.ino.merged.bin", plan.project)),
        elf: dirs.out.join(format!("{}.ino.elf", plan.project)),
    };
    for p in [&out.image, &out.elf] {
        if !p.is_file() {
            return Err(Error::MissingOutput(p.clone()));
        }
    }
    Ok(out)
}

/// Kills a process group when dropped, so cancelling a build stops the compilers
/// arduino-cli started, not just arduino-cli.
struct GroupGuard(Option<i32>);

impl Drop for GroupGuard {
    fn drop(&mut self) {
        if let Some(pgid) = self.0 {
            // SAFETY: kill(2) on a process group we created; harmless if it is gone.
            unsafe {
                libc::kill(-pgid, libc::SIGKILL);
            }
        }
    }
}

/// Run arduino-cli with `args` in its own process group (killed if the future is
/// dropped), streaming every output line to `log`. `name` labels the step in errors.
pub async fn run_arduino_cli(
    tools: &Toolchain,
    name: &'static str,
    args: Vec<OsString>,
    log: &mut impl FnMut(Stream, String),
) -> Result<(), Error> {
    step(tools, name, args, log).await
}

async fn step(
    tools: &Toolchain,
    name: &'static str,
    args: Vec<OsString>,
    log: &mut impl FnMut(Stream, String),
) -> Result<(), Error> {
    let mut child = Command::new(&tools.arduino_cli)
        .args(&args)
        .env("ARDUINO_DIRECTORIES_USER", &tools.sketchbook)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .kill_on_drop(true)
        .spawn()?;
    let mut guard = GroupGuard(child.id().and_then(|p| i32::try_from(p).ok()));

    // One reader task per stream; the channel closes when both streams have ended.
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<(Stream, String)>();
    if let Some(stdout) = child.stdout.take() {
        let tx = tx.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(l)) = lines.next_line().await {
                let _ = tx.send((Stream::Stdout, l));
            }
        });
    }
    if let Some(stderr) = child.stderr.take() {
        let tx = tx.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(l)) = lines.next_line().await {
                let _ = tx.send((Stream::Stderr, l));
            }
        });
    }
    drop(tx);
    while let Some((stream, line)) = rx.recv().await {
        log(stream, line);
    }
    let status = child.wait().await?;
    // Finished normally: nothing left to kill.
    guard.0 = None;
    if status.success() {
        Ok(())
    } else {
        Err(Error::StepFailed {
            step: name,
            status: status.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unsafe_ids() {
        for bad in ["", "../etc", "a/b", "-x", "A", "a b"] {
            assert!(safe_id(bad).is_err(), "{bad:?} accepted");
        }
        safe_id("i2c-scan").unwrap();
    }

    #[test]
    fn only_boards_without_a_usb_uart_put_serial_on_usb() {
        assert!(serial_on_usb("esp32c3-supermini"));
        for b in ["esp32", "esp32-doit", "esp32s3", "esp32c3"] {
            assert!(!serial_on_usb(b), "{b}");
        }
    }

    #[test]
    fn every_board_maps_to_an_esp32_fqbn() {
        for (_, fqbn, _) in BOARDS {
            assert!(fqbn.starts_with("esp32:esp32:"), "{fqbn}");
        }
    }
}
