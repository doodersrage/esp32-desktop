//! A supervised QEMU session: one child process with UART0 and QMP on Unix sockets.
//!
//! QEMU starts paused (`-S`). The session connects QMP and UART first, then sends
//! `cont`, so no early boot output is lost. The child is killed when the session is
//! dropped, and also if this process dies (`PR_SET_PDEATHSIG`).

use std::ffi::OsString;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use tokio::net::UnixStream;
use tokio::process::{Child, Command};
use tokio::time::Instant;

use crate::gpio::GpioLink;
use crate::qmp::Qmp;
use crate::uart::Uart;
use crate::{Chip, Error, image};

const READY_TIMEOUT: Duration = Duration::from_secs(10);
const QUIT_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone)]
pub struct SessionConfig {
    pub chip: Chip,
    /// Path to the QEMU binary for the chip's architecture.
    pub qemu: PathBuf,
    /// Merged flash image. It is padded and copied; the original is never written to.
    pub image: PathBuf,
    /// Open a GDB stub on 127.0.0.1 at this port.
    pub gdb_port: Option<u16>,
    /// Stay halted after start, for example to attach a debugger before the first instruction.
    pub start_paused: bool,
    /// Use this `-machine` value instead of the chip's default, e.g. `esp32-picsimlab`
    /// on lcgamboa's fork.
    pub machine: Option<String>,
    /// Further QEMU arguments, appended as-is. Each element is one argument; nothing is
    /// passed through a shell.
    pub extra_args: Vec<OsString>,
    /// Environment variables set for the QEMU process only, e.g. for a host program
    /// that wraps a QEMU library.
    pub env: Vec<(OsString, OsString)>,
    /// GPIOs to bridge through `esp32bench-qemu-host` (lcgamboa's library build only):
    /// the session passes them and a socket in its private directory to the host,
    /// then connects [`Session::gpio`]. Empty for none.
    pub bridge: Vec<u8>,
    /// A circuit (schema/circuit, as JSON) whose parts the host runs. Its pins must be
    /// in `bridge`. Needs lcgamboa's library build, like `bridge`.
    pub circuit: Option<String>,
    /// An eFuse image for the chip, copied into the session's directory (QEMU writes
    /// to it) and given to the machine's eFuse device. None: the machine's defaults.
    pub efuse: Option<Vec<u8>>,
}

/// Files a session uses, all inside one private runtime directory.
#[derive(Debug, Clone)]
pub struct SessionPaths {
    pub dir: PathBuf,
    pub flash: PathBuf,
    pub gpio: PathBuf,
    pub uart: PathBuf,
    pub qmp: PathBuf,
    pub stderr: PathBuf,
    pub efuse: PathBuf,
}

impl SessionPaths {
    pub fn in_dir(dir: PathBuf) -> Self {
        SessionPaths {
            flash: dir.join("flash.bin"),
            gpio: dir.join("gpio.sock"),
            uart: dir.join("uart0.sock"),
            qmp: dir.join("qmp.sock"),
            stderr: dir.join("qemu.stderr"),
            efuse: dir.join("efuse.bin"),
            dir,
        }
    }
}

/// QEMU's argument list for a session. Pure, so it can be tested without QEMU.
pub fn qemu_args(cfg: &SessionConfig, paths: &SessionPaths) -> Vec<OsString> {
    let mut args: Vec<OsString> = Vec::new();
    let mut push = |s: &str| args.push(s.into());
    push("-machine");
    push(cfg.machine.as_deref().unwrap_or(cfg.chip.machine()));
    push("-display");
    push("none");
    push("-monitor");
    push("none");
    // No network unless asked for: lcgamboa's fork models Wi-Fi, and downloaded
    // firmware must not reach the network without consent (rule 4).
    push("-nic");
    push("none");
    push("-S");

    let mut drive = OsString::from("file=");
    drive.push(&paths.flash);
    drive.push(",if=mtd,format=raw");
    let mut serial = OsString::from("unix:");
    serial.push(&paths.uart);
    serial.push(",server=on,wait=off");
    let mut qmp = OsString::from("unix:");
    qmp.push(&paths.qmp);
    qmp.push(",server=on,wait=off");

    args.extend([
        "-drive".into(),
        drive,
        "-serial".into(),
        serial,
        "-qmp".into(),
        qmp,
    ]);
    if let Some(port) = cfg.gdb_port {
        args.push("-gdb".into());
        args.push(format!("tcp:127.0.0.1:{port}").into());
    }
    if cfg.efuse.is_some() {
        let mut drive = OsString::from("file=");
        drive.push(&paths.efuse);
        drive.push(",if=none,format=raw,id=efuse");
        let device = match cfg.chip {
            Chip::Esp32 => "nvram.esp32.efuse",
            Chip::Esp32s3 => "nvram.esp32s3.efuse",
            Chip::Esp32c3 => "nvram.esp32c3.efuse",
        };
        args.extend([
            "-drive".into(),
            drive,
            "-global".into(),
            format!("driver={device},property=drive,value=efuse").into(),
        ]);
    }
    args.extend(cfg.extra_args.iter().cloned());
    args
}

pub struct Session {
    child: Child,
    pub qmp: Qmp,
    /// The pin link, when the session bridges GPIOs ([`SessionConfig::bridge`]).
    pub gpio: Option<GpioLink>,
    pub uart: Uart,
    paths: SessionPaths,
    gdb: Option<SocketAddr>,
}

impl Session {
    pub async fn start(cfg: SessionConfig) -> Result<Self, Error> {
        let data = tokio::fs::read(&cfg.image).await?;
        let info = image::inspect(cfg.chip, &data)?;
        let data = image::pad(data, &info);

        let paths = SessionPaths::in_dir(new_runtime_dir()?);
        let guard = DirGuard(Some(paths.dir.clone()));
        tokio::fs::write(&paths.flash, &data).await?;

        let stderr = std::fs::File::create(&paths.stderr)?;
        let mut cmd = Command::new(&cfg.qemu);
        cmd.args(qemu_args(&cfg, &paths))
            .envs(cfg.env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::from(stderr))
            .kill_on_drop(true);
        // SAFETY: prctl is async-signal-safe and touches no memory of the parent.
        // The death signal fires when the spawning thread exits; Tokio worker threads
        // live as long as the runtime, and kill_on_drop covers orderly shutdown.
        unsafe {
            cmd.pre_exec(|| {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let linked = !cfg.bridge.is_empty() || cfg.circuit.is_some();
        if linked {
            let pins: Vec<String> = cfg.bridge.iter().map(u8::to_string).collect();
            cmd.env("ESP32BENCH_GPIO_PINS", pins.join(","))
                .env("ESP32BENCH_GPIO_SOCK", &paths.gpio);
        }
        if let Some(efuse) = &cfg.efuse {
            tokio::fs::write(&paths.efuse, efuse).await?;
        }
        if let Some(circuit) = &cfg.circuit {
            let file = paths.dir.join("circuit.json");
            tokio::fs::write(&file, circuit).await?;
            cmd.env("ESP32BENCH_CIRCUIT", &file);
        }
        let mut child = cmd.spawn()?;

        let (qmp, uart) = match connect(&mut child, &paths, cfg.start_paused).await {
            Ok(c) => c,
            // A socket error during start-up usually means QEMU died; report why.
            Err(e) => return Err(exit_error(&mut child, &paths.stderr).await.unwrap_or(e)),
        };

        // The host binds its socket before QEMU starts, so it is there by now.
        let gpio = if !linked {
            None
        } else {
            match GpioLink::connect(&paths.gpio, Duration::from_secs(2)).await {
                Ok(g) => Some(g),
                Err(e) => return Err(exit_error(&mut child, &paths.stderr).await.unwrap_or(e)),
            }
        };

        guard.disarm();
        Ok(Session {
            child,
            qmp,
            uart,
            gpio,
            paths,
            gdb: cfg.gdb_port.map(|p| SocketAddr::from(([127, 0, 0, 1], p))),
        })
    }

    pub fn paths(&self) -> &SessionPaths {
        &self.paths
    }

    pub fn gdb_addr(&self) -> Option<SocketAddr> {
        self.gdb
    }

    pub async fn reset(&mut self) -> Result<(), Error> {
        self.qmp.execute("system_reset", None).await.map(|_| ())
    }

    pub async fn resume(&mut self) -> Result<(), Error> {
        self.qmp.execute("cont", None).await.map(|_| ())
    }

    /// Ask QEMU to quit, then kill it if it does not exit promptly.
    pub async fn shutdown(mut self) -> Result<(), Error> {
        let _ = self.qmp.execute("quit", None).await;
        if tokio::time::timeout(QUIT_TIMEOUT, self.child.wait())
            .await
            .is_err()
        {
            self.child.kill().await?;
        }
        Ok(())
    }

    /// Wait for QEMU to exit on its own.
    pub async fn wait(&mut self) -> Result<std::process::ExitStatus, Error> {
        Ok(self.child.wait().await?)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
        let _ = std::fs::remove_dir_all(&self.paths.dir);
    }
}

/// Removes the runtime directory if start-up fails part way.
struct DirGuard(Option<PathBuf>);

impl DirGuard {
    fn disarm(mut self) {
        self.0 = None;
    }
}

impl Drop for DirGuard {
    fn drop(&mut self) {
        if let Some(d) = self.0.take() {
            let _ = std::fs::remove_dir_all(d);
        }
    }
}

/// A private directory for one session under `$XDG_RUNTIME_DIR/esp32bench/`.
fn new_runtime_dir() -> Result<PathBuf, Error> {
    use std::os::unix::fs::DirBuilderExt;
    static COUNTER: AtomicU32 = AtomicU32::new(0);

    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join(crate::install::APP_DIR);
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&base)?;
    remove_stale_sessions(&base);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = base.join(format!("session-{}-{n}", std::process::id()));
    std::fs::DirBuilder::new().mode(0o700).create(&dir)?;
    Ok(dir)
}

/// Remove session directories whose owning process no longer runs, for example after
/// the core was killed with SIGKILL. QEMU itself is gone by then (`PR_SET_PDEATHSIG`).
/// Directories of live processes are left alone, so a reused PID only delays cleanup.
fn remove_stale_sessions(base: &Path) {
    let Ok(entries) = std::fs::read_dir(base) else {
        return;
    };
    for entry in entries.flatten() {
        if let Some(pid) = session_owner(&entry.file_name().to_string_lossy())
            && pid != std::process::id()
            && !Path::new("/proc").join(pid.to_string()).exists()
        {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// The owning PID of a `session-<pid>-<n>` directory name.
fn session_owner(name: &str) -> Option<u32> {
    let rest = name.strip_prefix("session-")?;
    let (pid, n) = rest.split_once('-')?;
    n.parse::<u32>().ok()?;
    pid.parse().ok()
}

/// Connect QMP and UART0, then let the CPU run unless the session starts paused.
async fn connect(
    child: &mut Child,
    paths: &SessionPaths,
    start_paused: bool,
) -> Result<(Qmp, Uart), Error> {
    let qmp_stream = connect_when_ready(child, &paths.qmp, &paths.stderr).await?;
    let mut qmp = Qmp::handshake(qmp_stream).await?;
    let uart_stream = connect_when_ready(child, &paths.uart, &paths.stderr).await?;
    let uart = Uart::new(uart_stream);
    if !start_paused {
        qmp.execute("cont", None).await?;
    }
    Ok((qmp, uart))
}

/// If QEMU has exited or exits within a moment, an error carrying its status and stderr.
async fn exit_error(child: &mut Child, stderr: &Path) -> Option<Error> {
    let status = tokio::time::timeout(Duration::from_millis(500), child.wait())
        .await
        .ok()?
        .ok()?;
    let stderr = std::fs::read(stderr).unwrap_or_default();
    Some(Error::QemuExited {
        status: status.to_string(),
        stderr: String::from_utf8_lossy(&stderr).trim().to_owned(),
    })
}

/// Retry connecting to a socket QEMU is about to create, failing fast if QEMU exits.
async fn connect_when_ready(
    child: &mut Child,
    sock: &Path,
    stderr: &Path,
) -> Result<UnixStream, Error> {
    let deadline = Instant::now() + READY_TIMEOUT;
    loop {
        if child.try_wait()?.is_some()
            && let Some(e) = exit_error(child, stderr).await
        {
            return Err(e);
        }
        if let Ok(s) = UnixStream::connect(sock).await {
            return Ok(s);
        }
        if Instant::now() >= deadline {
            return Err(Error::Timeout("QEMU sockets"));
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Write an executable script without this process ever holding it open for writing.
    /// Parallel tests fork; a child that inherited a write handle would make running the
    /// script fail with "Text file busy" (ETXTBSY).
    fn write_script(path: &std::path::Path, content: impl AsRef<[u8]>) {
        use std::io::Write;
        let mut child = std::process::Command::new("sh")
            .args(["-c", "cat > \"$1\" && chmod 755 \"$1\"", "sh"])
            .arg(path)
            .stdin(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(content.as_ref())
            .unwrap();
        assert!(child.wait().unwrap().success());
    }

    fn cfg(chip: Chip, gdb: Option<u16>) -> SessionConfig {
        SessionConfig {
            chip,
            qemu: "/q".into(),
            image: "/img.bin".into(),
            gdb_port: gdb,
            start_paused: false,
            machine: None,
            extra_args: Vec::new(),
            env: Vec::new(),
            bridge: Vec::new(),
            circuit: None,
            efuse: None,
        }
    }

    fn strs(v: Vec<OsString>) -> Vec<String> {
        v.into_iter().map(|s| s.into_string().unwrap()).collect()
    }

    #[test]
    fn builds_args_without_gdb() {
        let paths = SessionPaths::in_dir("/run/s".into());
        let args = strs(qemu_args(&cfg(Chip::Esp32c3, None), &paths));
        assert_eq!(
            args,
            [
                "-machine",
                "esp32c3",
                "-display",
                "none",
                "-monitor",
                "none",
                "-nic",
                "none",
                "-S",
                "-drive",
                "file=/run/s/flash.bin,if=mtd,format=raw",
                "-serial",
                "unix:/run/s/uart0.sock,server=on,wait=off",
                "-qmp",
                "unix:/run/s/qmp.sock,server=on,wait=off",
            ]
        );
    }

    #[test]
    fn efuse_image_is_a_file_in_the_session_directory() {
        let paths = SessionPaths::in_dir("/run/s".into());
        let mut c = cfg(Chip::Esp32c3, None);
        c.efuse = Some(vec![0; 1024]);
        let args = strs(qemu_args(&c, &paths));
        let at = args.iter().position(|a| a == "-global").unwrap();
        assert_eq!(
            args[at - 1],
            "file=/run/s/efuse.bin,if=none,format=raw,id=efuse"
        );
        assert_eq!(
            args[at + 1],
            "driver=nvram.esp32c3.efuse,property=drive,value=efuse"
        );
        c.efuse = None;
        assert!(
            !strs(qemu_args(&c, &paths))
                .iter()
                .any(|a| a.contains("efuse"))
        );
    }

    #[test]
    fn machine_override_and_extra_args_are_passed_verbatim() {
        let paths = SessionPaths::in_dir("/run/s".into());
        let mut c = cfg(Chip::Esp32, None);
        c.machine = Some("esp32-picsimlab".into());
        c.extra_args = vec!["-nic".into(), "user,model=esp32_wifi; rm -rf /".into()];
        let args = strs(qemu_args(&c, &paths));
        assert_eq!(args[1], "esp32-picsimlab");
        assert_eq!(
            args[args.len() - 2..],
            ["-nic", "user,model=esp32_wifi; rm -rf /"]
        );
    }

    #[test]
    fn no_network_by_default() {
        let paths = SessionPaths::in_dir("/tmp/x".into());
        let args = qemu_args(&cfg(Chip::Esp32, None), &paths);
        let i = args.iter().position(|a| a == "-nic").unwrap();
        assert_eq!(args[i + 1], "none");
    }

    #[test]
    fn gdb_stub_binds_loopback_only() {
        let paths = SessionPaths::in_dir("/run/s".into());
        let args = strs(qemu_args(&cfg(Chip::Esp32, Some(3333)), &paths));
        let i = args.iter().position(|a| a == "-gdb").unwrap();
        assert_eq!(args[i + 1], "tcp:127.0.0.1:3333");
    }

    #[tokio::test]
    async fn reports_qemu_stderr_when_it_dies_at_start() {
        let dir = tempfile::tempdir().unwrap();
        let fake = dir.path().join("fake-qemu");
        write_script(&fake, "#!/bin/sh\necho 'Assertion failed' >&2\nexit 3\n");
        let image = dir.path().join("flash.bin");
        let mut data = vec![0xFF; 8192];
        data[0x1000..0x1004].copy_from_slice(&[0xE9, 3, 2, 0x20]);
        std::fs::write(&image, data).unwrap();

        let mut c = cfg(Chip::Esp32, None);
        c.qemu = fake;
        c.image = image;
        match Session::start(c).await {
            Err(Error::QemuExited { status, stderr }) => {
                assert!(status.contains('3'), "status: {status}");
                assert_eq!(stderr, "Assertion failed");
            }
            Err(e) => panic!("expected QemuExited, got {e}"),
            Ok(_) => panic!("expected QemuExited, got a session"),
        }
    }

    #[test]
    fn parses_session_dir_names() {
        assert_eq!(session_owner("session-1234-0"), Some(1234));
        assert_eq!(session_owner("session-1234"), None);
        assert_eq!(session_owner("session-x-0"), None);
        assert_eq!(session_owner("other-1234-0"), None);
    }

    #[test]
    fn removes_only_dead_sessions() {
        let base = tempfile::tempdir().unwrap();
        // PID 0x7fff_fff0 is above the kernel's maximum, so it is never running.
        let dead = base.path().join("session-2147483632-0");
        let live = base
            .path()
            .join(format!("session-{}-99", std::process::id()));
        let other = base.path().join("unrelated");
        for d in [&dead, &live, &other] {
            std::fs::create_dir(d).unwrap();
        }
        remove_stale_sessions(base.path());
        assert!(!dead.exists());
        assert!(live.exists());
        assert!(other.exists());
    }

    #[test]
    fn runtime_dirs_are_private_and_unique() {
        use std::os::unix::fs::PermissionsExt;
        let a = new_runtime_dir().unwrap();
        let b = new_runtime_dir().unwrap();
        assert_ne!(a, b);
        let mode = std::fs::metadata(&a).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
        std::fs::remove_dir(a).unwrap();
        std::fs::remove_dir(b).unwrap();
    }
}
