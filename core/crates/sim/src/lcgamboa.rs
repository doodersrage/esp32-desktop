//! lcgamboa's QEMU fork: the pin, where the build lives, and building it from source.
//!
//! The fork has no releases, so it is pinned to a commit and built here, without root:
//! the ordinary executables (out of tree, in `src/build-exe/`) and the library build
//! that exposes the GPIO and I2C hooks (the fork's own `build_libqemu-esp32.sh`, which
//! builds in `src/build/`). The ROMs stay in the source tree's `pc-bios/`.
//!
//! Layout: `$XDG_DATA_HOME/esp32bench/qemu/lcgamboa-<commit>/` with `src/` and an
//! `installed` marker written last, so a half-finished build is never used. The build
//! runs in a staging directory renamed into place at the end.
//! `ESP32BENCH_LCGAMBOA_DIR` points at another directory with the same layout.

use esp32bench_distro::{Distro, Need};
use std::path::{Path, PathBuf};
use std::process::Stdio;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use crate::{Arch, Error, install};

/// The pinned commit (2026-11-14), QEMU 9.2.2. Chosen in phase 0: docs/qemu-forks.md.
pub const COMMIT: &str = "885a6e7061fb70fac78a074a719423810bc19a17";
pub const REPO: &str = "https://github.com/lcgamboa/qemu.git";
pub const DIR_ENV: &str = "ESP32BENCH_LCGAMBOA_DIR";
const MARKER: &str = "installed";
/// Bumped whenever the build steps change, so older builds are rebuilt.
/// 2: the library is built with pixman, for the virtual RGB panel and screendump.
/// 3: patches as files (PATCHES); esp32-picsimlab reads "QEMU" for the RGB driver.
/// 4: the CPU clock change fix (Arduino builds panicked at start-up).
/// 5: S3 ADC and LEDC registers, C3 LEDC readback (sketches hung or aborted).
/// 6: the C3's USB-Serial-JTAG console (the C3 SuperMini's Serial).
/// 7: RMT transmission on all three chips (WS2812 writes never finished).
/// 8: light and deep sleep on a timer (sleep hung or ended in a watchdog reset).
/// 9: an optimised library build (it was -O0) and quicker eFuse reads.
/// 10: coprocessors (the FPU) enabled from reset (floats in global constructors).
/// 11: RMT symbols and transmission ends reported to the host (WS2812 strip parts).
/// 12: a Wi-Fi radio on the S3, from the C3's models.
/// 13: light sleep stops the watchdogs (ESP-IDF's light sleep panicked); cheap ESP32
///     cache toggles (SPIFFS_check never finished); ESP32 eFuse operations end only
///     once seen busy (on a loaded host the ROM reset the chip at boot).
/// 14: the S3 takes the library's host hooks (pins, I2C, analog inputs), with its own
///     GPIO register map and two I2C controllers; its RTC slow clock runs at the
///     calibrated 136 kHz (light sleep woke milliseconds late).
pub const RECIPE: u32 = 14;

/// Changes esp32bench makes to the pinned commit, in order. Each file says what and
/// why. Kept in `core/crates/sim/patches/` and built into the program, so an installed
/// app can build the fork without the repo.
pub const PATCHES: [(&str, &str); 16] = [
    (
        "0001.patch",
        include_str!("../patches/0001-build-library-with-pixman.patch"),
    ),
    (
        "0002.patch",
        include_str!("../patches/0002-esp32-picsimlab-qemu-signature.patch"),
    ),
    (
        "0003.patch",
        include_str!("../patches/0003-cpu-clock-changes.patch"),
    ),
    (
        "0004.patch",
        include_str!("../patches/0004-esp32s3-adc-and-ledc-registers.patch"),
    ),
    (
        "0005.patch",
        include_str!("../patches/0005-esp32c3-ledc-readback.patch"),
    ),
    (
        "0006.patch",
        include_str!("../patches/0006-esp32c3-usb-serial-jtag-console.patch"),
    ),
    (
        "0007.patch",
        include_str!("../patches/0007-rmt-transmitter.patch"),
    ),
    ("0008.patch", include_str!("../patches/0008-sleep.patch")),
    (
        "0009.patch",
        include_str!("../patches/0009-faster-library-and-efuse.patch"),
    ),
    (
        "0010.patch",
        include_str!("../patches/0010-coprocessors-enabled-at-reset.patch"),
    ),
    (
        "0011.patch",
        include_str!("../patches/0011-rmt-symbols-to-the-host.patch"),
    ),
    (
        "0012.patch",
        include_str!("../patches/0012-esp32s3-wifi-radio.patch"),
    ),
    (
        "0013.patch",
        include_str!("../patches/0013-light-sleep-watchdogs-and-esp32-cache.patch"),
    ),
    (
        "0014.patch",
        include_str!("../patches/0014-esp32-efuse-busy-before-done.patch"),
    ),
    (
        "0015.patch",
        include_str!("../patches/0015-esp32s3-host-hooks.patch"),
    ),
    (
        "0016.patch",
        include_str!("../patches/0016-esp32s3-rtc-slow-clock.patch"),
    ),
];

fn marker_text() -> String {
    format!("{COMMIT}\nrecipe {RECIPE}\n")
}
const TARGETS: &str = "xtensa-softmmu,riscv32-softmmu";

/// The short form shown to people.
pub fn short_commit() -> &'static str {
    &COMMIT[..10]
}

/// Where the pinned build lives, whether or not it exists.
pub fn root() -> Result<PathBuf, Error> {
    if let Some(d) = std::env::var_os(DIR_ENV).filter(|d| !d.is_empty()) {
        return Ok(PathBuf::from(d));
    }
    Ok(install::data_dir()?
        .join("qemu")
        .join(format!("lcgamboa-{}", short_commit())))
}

/// A finished build of the pinned commit.
#[derive(Debug, Clone, PartialEq)]
pub struct Build {
    pub root: PathBuf,
}

impl Build {
    /// The build under `root`, if it finished (the marker names the pinned commit).
    pub fn at(root: &Path) -> Option<Build> {
        let marker = std::fs::read_to_string(root.join(MARKER)).ok()?;
        (marker == marker_text()).then(|| Build {
            root: root.to_owned(),
        })
    }

    /// Whether `root` holds a finished build from another commit or an older recipe.
    pub fn outdated_at(root: &Path) -> bool {
        root.join(MARKER).is_file() && Build::at(root).is_none()
    }

    /// The installed build, if any.
    pub fn find() -> Option<Build> {
        Build::at(&root().ok()?)
    }

    pub fn executable(&self, arch: Arch) -> PathBuf {
        self.root.join("src/build-exe").join(arch.binary_name())
    }

    /// The library build (`libqemu-xtensa.so`, `libqemu-riscv32.so`).
    pub fn library(&self, arch: Arch) -> PathBuf {
        self.root
            .join("src/build")
            .join(format!("libqemu-{}.so", arch.as_str()))
    }

    /// ROM directory to pass with `-L`.
    pub fn pc_bios(&self) -> PathBuf {
        self.root.join("src/pc-bios")
    }

    /// Arguments every run of this build needs: the ROMs live in the source tree,
    /// not beside the out-of-tree executables.
    pub fn qemu_args(&self) -> Vec<std::ffi::OsString> {
        vec!["-L".into(), self.pc_bios().into()]
    }
}

/// An eFuse image for the fork's ESP32-C3 with what its built-in defaults set (they
/// apply only without a file, and the C3's Wi-Fi radio needs a file): chip revision
/// v0.3, eFuse block revision v1.3, and the same MAC address, c4:0a:24:fe:10:01.
/// 1024 bytes, from EFUSE_RD_WR_DIS (register 0x2C) on.
pub fn esp32c3_efuse() -> Vec<u8> {
    let mut b = vec![0u8; 1024];
    // EFUSE_RD_MAC_SPI_SYS_0/1 (0x44): the MAC, lowest byte first.
    b[24..30].copy_from_slice(&[0x01, 0x10, 0xfe, 0x24, 0x0a, 0xc4]);
    // EFUSE_RD_MAC_SPI_SYS_3 (0x50): WAFER_VER_MINOR_LO = 3, BLK_VERSION_MINOR = 3.
    b[36..40].copy_from_slice(&((3u32 << 18) | (3u32 << 24)).to_le_bytes());
    // EFUSE_RD_SYS_PART1_DATA4 (0x6C): BLK_VERSION_MAJOR = 1.
    b[64..68].copy_from_slice(&1u32.to_le_bytes());
    b
}

/// An eFuse image for the fork's ESP32: blank, as its built-in defaults are (so the
/// same default MAC address), except for the ADC reference voltage factory-tested
/// chips have burnt in. Without it ESP-IDF's ADC calibration fails, and Arduino's
/// analogReadMilliVolts() returns 0. ADC_VREF is BLK0 bits 136 to 140 (word 4, bits
/// 8 to 12): a sign-magnitude offset from 1100 mV in 7 mV steps, where all zeros
/// means "not burnt". The sign bit alone is a burnt offset of 0: exactly 1100 mV.
/// 1024 bytes, read from EFUSE_BLK0_RDATA0 on (the fork reads its first 124).
pub fn esp32_efuse() -> Vec<u8> {
    let mut b = vec![0u8; 1024];
    b[16..20].copy_from_slice(&(0x10u32 << 8).to_le_bytes());
    b
}

/// An eFuse image for the fork's ESP32-S3: blank, as the model is without one,
/// except BLK_VERSION_MAJOR = 1 (BLOCK2 bits 128-129, EFUSE_RD_SYS_PART1_DATA4 at
/// 0x6C), which tells ESP-IDF the chip carries ADC calibration ("ADC calib V1"), as
/// factory-tested S3s do. The calibration offsets themselves are left 0, so ESP-IDF
/// uses its typical values. Without it ESP-IDF's curve fitting refuses to calibrate,
/// and Arduino's analogReadMilliVolts() returns 0. Same layout as the C3's: the
/// file starts at EFUSE_RD_WR_DIS (0x2C).
pub fn esp32s3_efuse() -> Vec<u8> {
    let mut b = vec![0u8; 1024];
    b[64..68].copy_from_slice(&1u32.to_le_bytes());
    b
}

/// The machine to run each chip on with this fork. Its plain `esp32` machine aborts
/// at start-up at the pinned commit. The PICSimLab machines have the GPIO and I2C
/// hooks; the S3 has no such machine, and its own takes the hooks (patch 0015,
/// docs/qemu-forks.md).
pub fn machine(chip: crate::Chip) -> &'static str {
    match chip {
        crate::Chip::Esp32 => "esp32-picsimlab",
        crate::Chip::Esp32s3 => "esp32s3",
        crate::Chip::Esp32c3 => "esp32c3-picsimlab",
    }
}

/// A tool or library the build needs: the command or pkg-config name checked, and
/// what provides it (`esp32bench_distro` names the package on each distribution).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Requirement {
    pub what: &'static str,
    pub need: Need,
}

/// Commands on PATH the build runs.
const COMMANDS: &[(&str, Need)] = &[
    ("git", Need::Git),
    ("make", Need::Make),
    ("cc", Need::Gcc),
    ("pkg-config", Need::PkgConfig),
    ("ninja", Need::Ninja),
    ("python3", Need::Python),
    ("perl", Need::Perl),
    ("sed", Need::Sed),
];

/// Libraries, by pkg-config name.
const LIBRARIES: &[(&str, Need)] = &[
    ("glib-2.0", Need::Glib),
    ("pixman-1", Need::Pixman),
    ("libgcrypt", Need::Libgcrypt),
    ("slirp", Need::Slirp),
];

fn on_path(cmd: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|p| {
        std::env::split_paths(&p).any(|d| {
            let f = d.join(cmd);
            f.is_file() && is_executable(&f)
        })
    })
}

fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    p.metadata()
        .is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
}

/// What is missing to build the fork. Runs `pkg-config --exists` only; no network.
pub async fn missing_requirements() -> Vec<Requirement> {
    let mut out: Vec<Requirement> = COMMANDS
        .iter()
        .filter(|(c, _)| !on_path(c))
        .map(|&(what, need)| Requirement { what, need })
        .collect();
    // QEMU's configure makes a Python virtual environment; Debian ships venv apart.
    if on_path("python3")
        && !Command::new("python3")
            .args(["-c", "import venv, ensurepip"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .is_ok_and(|s| s.success())
    {
        out.push(Requirement {
            what: "python3 venv",
            need: Need::PythonVenv,
        });
    }
    let have_pkg_config = on_path("pkg-config");
    for &(lib, need) in LIBRARIES {
        let found = have_pkg_config
            && Command::new("pkg-config")
                .args(["--exists", lib])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .await
                .is_ok_and(|s| s.success());
        if !found {
            out.push(Requirement { what: lib, need });
        }
    }
    out
}

/// The command a person runs to install what is missing on `distro` (the app never
/// runs it).
pub fn install_hint(distro: Distro, missing: &[Requirement]) -> Option<String> {
    let needs: Vec<Need> = missing.iter().map(|r| r.need).collect();
    distro.install_hint(&needs)
}

/// One step of the build: a program, its arguments and where it runs.
#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    pub what: &'static str,
    pub program: &'static str,
    pub args: Vec<String>,
    /// Relative to the staging directory.
    pub dir: &'static str,
}

/// The build, in order. Fixed arguments only: nothing here comes from a project.
pub fn steps(jobs: usize) -> Vec<Step> {
    let s = |what, program, args: &[&str], dir| Step {
        what,
        program,
        args: args.iter().map(|a| (*a).to_owned()).collect(),
        dir,
    };
    let jobs = format!("-j{jobs}");
    vec![
        s(
            "Preparing the source directory",
            "git",
            &["init", "-q", "src"],
            ".",
        ),
        s(
            "Downloading the pinned commit",
            "git",
            &["fetch", "--depth", "1", REPO, COMMIT],
            "src",
        ),
        s(
            "Checking it out",
            "git",
            &[
                "-c",
                "advice.detachedHead=false",
                "checkout",
                "-q",
                "FETCH_HEAD",
            ],
            "src",
        ),
        // Our patches (PATCHES), written next to src/ before the build.
        s(
            "Applying esp32bench's patches",
            "git",
            &[
                "apply",
                "--verbose",
                "../patches/0001.patch",
                "../patches/0002.patch",
                "../patches/0003.patch",
                "../patches/0004.patch",
                "../patches/0005.patch",
                "../patches/0006.patch",
                "../patches/0007.patch",
                "../patches/0008.patch",
                "../patches/0009.patch",
                "../patches/0010.patch",
                "../patches/0011.patch",
                "../patches/0012.patch",
                "../patches/0013.patch",
                "../patches/0014.patch",
                "../patches/0015.patch",
                "../patches/0016.patch",
            ],
            "src",
        ),
        s(
            "Creating the executables' build directory",
            "mkdir",
            &["build-exe"],
            "src",
        ),
        s(
            "Configuring the executables",
            "../configure",
            &[
                &format!("--target-list={TARGETS}"),
                "--enable-slirp",
                "--enable-gcrypt",
                "--disable-werror",
            ],
            "src/build-exe",
        ),
        s(
            "Building the executables",
            "make",
            &[&jobs],
            "src/build-exe",
        ),
        // The fork's script configures in the source tree (build/) and relinks the
        // executables as shared libraries.
        s(
            "Building the GPIO and I2C library",
            "sh",
            &["build_libqemu-esp32.sh"],
            "src",
        ),
    ]
}

/// Removes a directory when dropped, unless disarmed.
struct RemoveOnDrop(Option<PathBuf>);

impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        if let Some(dir) = self.0.take() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

/// Kills a step's whole process group (make and its compilers) when dropped early.
struct GroupGuard(Option<u32>);

impl Drop for GroupGuard {
    fn drop(&mut self) {
        if let Some(pid) = self.0.and_then(|p| i32::try_from(p).ok()) {
            // SAFETY: killpg on the group this guard's child leads.
            unsafe {
                libc::killpg(pid, libc::SIGKILL);
            }
        }
    }
}

async fn run_step(base: &Path, step: &Step, log: &mut impl FnMut(String)) -> Result<(), Error> {
    let fail = |detail: String| Error::Build {
        step: step.what.to_owned(),
        detail,
    };
    log(format!("{}…", step.what));
    let mut child = Command::new(step.program)
        .args(&step.args)
        .current_dir(base.join(step.dir))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("LC_ALL", "C")
        .process_group(0)
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| fail(format!("could not run {}: {e}", step.program)))?;
    let mut guard = GroupGuard(child.id());
    let (Some(out), Some(err)) = (child.stdout.take(), child.stderr.take()) else {
        return Err(fail("no output pipes".into()));
    };
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    for pipe in [
        Box::new(out) as Box<dyn tokio::io::AsyncRead + Unpin + Send>,
        Box::new(err),
    ] {
        let tx = tx.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(pipe).lines();
            while let Ok(Some(l)) = lines.next_line().await {
                if tx.send(l).is_err() {
                    break;
                }
            }
        });
    }
    drop(tx);
    // The last lines say why a step failed.
    let mut tail: std::collections::VecDeque<String> = std::collections::VecDeque::new();
    while let Some(line) = rx.recv().await {
        if tail.len() == 20 {
            tail.pop_front();
        }
        tail.push_back(line.clone());
        log(line);
    }
    let status = child.wait().await.map_err(|e| fail(e.to_string()))?;
    guard.0 = None; // it exited by itself
    if status.success() {
        Ok(())
    } else {
        let last: Vec<String> = tail.into_iter().collect();
        Err(fail(format!("{status}; last output:\n{}", last.join("\n"))))
    }
}

/// Build the pinned commit into [`root`], reporting progress lines to `log`. Takes
/// several minutes and about 2 GB of disk. Dropping the future stops the build and
/// leaves the previous install (if any) untouched.
pub async fn build(mut log: impl FnMut(String)) -> Result<Build, Error> {
    let missing = missing_requirements().await;
    if !missing.is_empty() {
        let what: Vec<&str> = missing.iter().map(|r| r.what).collect();
        let hint = install_hint(Distro::detect(), &missing).unwrap_or_default();
        return Err(Error::Build {
            step: "Checking build tools".into(),
            detail: format!("missing {}; {hint}", what.join(", ")),
        });
    }
    let root = root()?;
    let parent = root.parent().ok_or(Error::NoDataDir)?;
    tokio::fs::create_dir_all(parent).await?;
    let staging = parent.join(format!(
        ".{}.partial",
        root.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("lcgamboa")
    ));
    if staging.exists() {
        tokio::fs::remove_dir_all(&staging).await?;
    }
    tokio::fs::create_dir_all(staging.join("patches")).await?;
    for (name, text) in PATCHES {
        tokio::fs::write(staging.join("patches").join(name), text).await?;
    }
    // A failed or cancelled build removes its staging directory (most of 2 GB).
    let mut cleanup = RemoveOnDrop(Some(staging.clone()));
    let jobs = std::thread::available_parallelism().map_or(2, |n| n.get());
    for step in steps(jobs) {
        run_step(&staging, &step, &mut log).await?;
    }
    for arch in [Arch::Xtensa, Arch::Riscv32] {
        let b = Build {
            root: staging.clone(),
        };
        for p in [b.executable(arch), b.library(arch)] {
            if !p.is_file() {
                return Err(Error::Build {
                    step: "Checking the build".into(),
                    detail: format!("{} was not built", p.display()),
                });
            }
        }
    }
    tokio::fs::write(staging.join(MARKER), marker_text()).await?;
    if root.exists() {
        tokio::fs::remove_dir_all(&root).await?;
    }
    tokio::fs::rename(&staging, &root).await?;
    cleanup.0 = None;
    log(format!(
        "Installed lcgamboa's QEMU {} in {}",
        short_commit(),
        root.display()
    ));
    Ok(Build { root })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_build_counts_only_with_the_pinned_marker() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(Build::at(dir.path()), None);
        std::fs::write(dir.path().join(MARKER), "0123456789abcdef\n").unwrap();
        assert_eq!(Build::at(dir.path()), None, "another commit");
        std::fs::write(dir.path().join(MARKER), format!("{COMMIT}\n")).unwrap();
        assert_eq!(Build::at(dir.path()), None, "an older recipe");
        std::fs::write(dir.path().join(MARKER), marker_text()).unwrap();
        let b = Build::at(dir.path()).unwrap();
        assert!(
            b.executable(Arch::Xtensa)
                .ends_with("src/build-exe/qemu-system-xtensa")
        );
        assert!(
            b.library(Arch::Riscv32)
                .ends_with("src/build/libqemu-riscv32.so")
        );
        assert!(b.pc_bios().ends_with("src/pc-bios"));
    }

    #[test]
    fn steps_fetch_the_pinned_commit_and_use_fixed_arguments() {
        let steps = steps(8);
        let fetch = steps
            .iter()
            .find(|s| s.args.first().is_some_and(|a| a == "fetch"))
            .unwrap();
        assert_eq!(fetch.args, ["fetch", "--depth", "1", REPO, COMMIT]);
        assert!(
            steps
                .iter()
                .any(|s| s.program == "make" && s.args == ["-j8"])
        );
        assert_eq!(steps.last().unwrap().args, ["build_libqemu-esp32.sh"]);
        let apply = steps
            .iter()
            .find(|s| s.args.first().is_some_and(|a| a == "apply"))
            .unwrap();
        for (name, _) in PATCHES {
            assert!(
                apply.args.iter().any(|a| a.ends_with(name)),
                "{name} not applied"
            );
        }
        assert_eq!(COMMIT.len(), 40);
    }

    #[test]
    fn hint_names_each_distributions_packages() {
        let missing = [
            Requirement {
                what: "ninja",
                need: Need::Ninja,
            },
            Requirement {
                what: "slirp",
                need: Need::Slirp,
            },
        ];
        assert_eq!(
            install_hint(Distro::Arch, &missing).unwrap(),
            "sudo pacman -S --needed ninja libslirp"
        );
        assert_eq!(
            install_hint(Distro::Debian, &missing).unwrap(),
            "sudo apt install ninja-build libslirp-dev"
        );
        assert_eq!(install_hint(Distro::Fedora, &[]), None);
    }
}
