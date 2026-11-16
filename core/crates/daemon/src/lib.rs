//! The core daemon's method table and socket setup. `main.rs` only parses arguments
//! and handles signals; everything here is testable without a running daemon.

use std::collections::HashMap;
use std::fs::File;
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use esp32bench_rpc::{Ctx, Router, RpcError, code, parse_params};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::net::UnixListener;

/// Protocol version, as in `schema/rpc/index.json`. A test keeps them equal.
pub const PROTOCOL: &str = "0.6.0";
pub const NAME: &str = "esp32benchd";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Daemon-wide state shared by all connections.
pub struct State {
    /// Project library cache.
    pub library: esp32bench_library::Cache,
    /// Where library.refresh fetches from.
    pub library_source: String,
    /// One refresh at a time, so two never race on the cache file.
    refresh_lock: tokio::sync::Mutex<()>,
    /// Consent tokens handed out for builds (rule 4).
    pub consent: esp32bench_build::Consent,
    /// Build folders live under here, one per project and board.
    pub builds_root: PathBuf,
    /// Private Arduino sketchbook for installed libraries.
    pub sketchbook: PathBuf,
    /// arduino-cli to use instead of `$ESP32BENCH_ARDUINO_CLI` or PATH (tests).
    pub arduino_cli: Option<PathBuf>,
    /// One build at a time.
    build_lock: tokio::sync::Mutex<()>,
    /// Open serial ports by (connection, subscription): subscription ids are only
    /// unique within a connection.
    serial_ports: std::sync::Mutex<HashMap<(u64, u64), SharedPort>>,
    /// Ports serial.open may open instead of those device.list reports (tests).
    pub serial_allow: Option<Vec<PathBuf>>,
}

type SharedPort = Arc<std::sync::Mutex<esp32bench_serial::Port>>;

/// Removes a serial port from the table when its subscription ends, however it ends,
/// which drops the Port and closes the device.
struct PortEntry {
    state: Arc<State>,
    key: (u64, u64),
}

impl Drop for PortEntry {
    fn drop(&mut self) {
        let mut ports = self
            .state
            .serial_ports
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        ports.remove(&self.key);
    }
}

impl State {
    pub fn new(
        library: esp32bench_library::Cache,
        library_source: String,
        builds_root: PathBuf,
        sketchbook: PathBuf,
    ) -> Self {
        State {
            library,
            library_source,
            refresh_lock: tokio::sync::Mutex::new(()),
            consent: esp32bench_build::Consent::new(),
            builds_root,
            sketchbook,
            arduino_cli: None,
            build_lock: tokio::sync::Mutex::new(()),
            serial_ports: std::sync::Mutex::new(HashMap::new()),
            serial_allow: None,
        }
    }

    /// Cache in `$XDG_CACHE_HOME/esp32bench`; source from `$ESP32BENCH_MANIFEST` or the site.
    pub fn from_env() -> io::Result<Self> {
        let dir = esp32bench_library::cache_dir()
            .map_err(|e| io::Error::new(io::ErrorKind::NotFound, e.to_string()))?;
        let sketchbook = esp32bench_build::default_sketchbook().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "no data directory: set XDG_DATA_HOME or HOME",
            )
        })?;
        Ok(Self::new(
            esp32bench_library::Cache::new(dir.clone()),
            esp32bench_library::default_source(),
            dir.join("builds"),
            sketchbook,
        ))
    }
}

/// Every method the daemon serves. A test checks this against `schema/rpc/`.
pub fn router(state: Arc<State>) -> Router {
    let (s1, s2, s3, s4) = (state.clone(), state.clone(), state.clone(), state.clone());
    let (s5, s6, s7) = (state.clone(), state.clone(), state);
    Router::new()
        .method("serial.open", move |p, ctx| {
            let s = s5.clone();
            async move { serial_open(p, ctx, s).await }
        })
        .method("serial.write", move |p, ctx| {
            let s = s6.clone();
            async move { serial_write(p, ctx, s).await }
        })
        .method("serial.reset", move |p, ctx| {
            let s = s7.clone();
            async move { serial_reset(p, ctx, s).await }
        })
        .method("build.start", move |p, ctx| {
            let s = s4.clone();
            async move { build_start(p, ctx, s).await }
        })
        .method("core.hello", |p, _| async move { core_hello(p) })
        .method("job.cancel", |p, ctx| async move { job_cancel(p, ctx) })
        .method("library.list", move |p, _| {
            let s = s1.clone();
            async move { library_list(p, s).await }
        })
        .method("library.get", move |p, _| {
            let s = s3.clone();
            async move { library_get(p, s).await }
        })
        .method("library.refresh", move |p, ctx| {
            let s = s2.clone();
            async move { library_refresh(p, ctx, s) }
        })
        .method("core.unsubscribe", |p, ctx| async move {
            core_unsubscribe(p, ctx)
        })
        .method("device.list", |p, _| async move { device_list(p).await })
        .method("device.watch", |p, ctx| async move {
            device_watch(p, ctx).await
        })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HelloParams {
    #[serde(default)]
    #[allow(dead_code)] // logged once there is logging
    client: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    protocol: Option<String>,
}

fn core_hello(params: Value) -> Result<Value, RpcError> {
    let _: HelloParams = parse_params(params)?;
    Ok(json!({ "name": NAME, "version": VERSION, "protocol": PROTOCOL }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UnsubscribeParams {
    subscription: u64,
}

fn core_unsubscribe(params: Value, ctx: Ctx) -> Result<Value, RpcError> {
    let p: UnsubscribeParams = parse_params(params)?;
    if ctx.unsubscribe(p.subscription) {
        Ok(json!({}))
    } else {
        Err(RpcError::new(
            code::NOT_FOUND,
            format!("no subscription {} on this connection", p.subscription),
        ))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct JobParams {
    job: u64,
}

/// Jobs are subscriptions that end themselves; cancelling one aborts its task.
fn job_cancel(params: Value, ctx: Ctx) -> Result<Value, RpcError> {
    let p: JobParams = parse_params(params)?;
    if ctx.unsubscribe(p.job) {
        Ok(json!({}))
    } else {
        Err(RpcError::new(
            code::NOT_FOUND,
            format!("no job {} on this connection", p.job),
        ))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NoParams {}

async fn library_list(params: Value, state: Arc<State>) -> Result<Value, RpcError> {
    let _: NoParams = parse_params(params)?;
    let manifest = state
        .library
        .load()
        .await
        .map_err(|e| RpcError::new(code::FAILED, e.to_string()))?;
    let Some(m) = manifest else {
        return Ok(json!({ "available": false, "source": state.library_source, "projects": [] }));
    };
    let mut out = json!({
        "available": true,
        "source": state.library_source,
        "version": m.version,
        "generated": m.generated,
        "projects": m.summaries(),
    });
    if let Some(t) = esp32bench_library::modified_secs(&state.library.path()) {
        out["fetchedAt"] = json!(t);
    }
    Ok(out)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GetParams {
    id: String,
}

async fn library_get(params: Value, state: Arc<State>) -> Result<Value, RpcError> {
    let p: GetParams = parse_params(params)?;
    let manifest = state
        .library
        .load()
        .await
        .map_err(|e| RpcError::new(code::FAILED, e.to_string()))?
        .ok_or_else(|| {
            RpcError::new(
                code::NOT_FOUND,
                "the project library has not been downloaded",
            )
        })?;
    let project = manifest.project(&p.id).ok_or_else(|| {
        RpcError::new(
            code::NOT_FOUND,
            format!("no project `{}` in the library", p.id),
        )
    })?;
    let boards: Vec<_> = manifest
        .boards
        .iter()
        .filter(|b| project.builds.iter().any(|x| x.board == b.id))
        .collect();
    Ok(json!({ "project": project, "boards": boards }))
}

fn library_refresh(params: Value, ctx: Ctx, state: Arc<State>) -> Result<Value, RpcError> {
    let _: NoParams = parse_params(params)?;
    let job = ctx.subscribe(move |sub| async move {
        let _one_at_a_time = state.refresh_lock.lock().await;
        let event = match state.library.refresh(&state.library_source).await {
            Ok(m) => json!({ "ok": true, "projects": m.projects.len(), "version": m.version }),
            Err(e) => json!({ "ok": false, "error": e.to_string() }),
        };
        sub.send("library.refreshed", event);
    });
    Ok(json!({ "job": job }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BuildParams {
    project: String,
    board: String,
    #[serde(default)]
    consent: Option<String>,
}

fn build_error(e: esp32bench_build::Error) -> RpcError {
    use esp32bench_build::Error as E;
    let code = match &e {
        E::NoProject(_) | E::NoBuild { .. } => code::NOT_FOUND,
        E::BadId(_) => code::INVALID_PARAMS,
        E::NoArduinoCli => code::UNAVAILABLE,
        _ => code::FAILED,
    };
    RpcError::new(code, e.to_string())
}

async fn build_start(params: Value, ctx: Ctx, state: Arc<State>) -> Result<Value, RpcError> {
    let p: BuildParams = parse_params(params)?;
    let manifest = state
        .library
        .load()
        .await
        .map_err(|e| RpcError::new(code::FAILED, e.to_string()))?
        .ok_or_else(|| {
            RpcError::new(
                code::NOT_FOUND,
                "the project library has not been downloaded",
            )
        })?;
    let (plan, code) =
        esp32bench_build::plan(&manifest, &p.project, &p.board).map_err(build_error)?;
    // Check the toolchain before asking for consent, so nobody agrees to a build that
    // cannot run.
    let tools = match &state.arduino_cli {
        Some(cli) => esp32bench_build::Toolchain {
            arduino_cli: cli.clone(),
            sketchbook: state.sketchbook.clone(),
        },
        None => {
            esp32bench_build::Toolchain::detect(state.sketchbook.clone()).map_err(build_error)?
        }
    };

    // Library projects are downloaded, so they never build without consent (rule 4).
    let granted = p
        .consent
        .as_deref()
        .is_some_and(|t| state.consent.redeem(t, &plan));
    if !granted {
        let token = state
            .consent
            .issue(&plan)
            .map_err(|e| RpcError::internal(e.to_string()))?;
        let message = if p.consent.is_some() {
            "the consent token is not valid for this build; ask the user again"
        } else {
            "this project was downloaded; ask the user before building it"
        };
        return Err(RpcError::new(code::CONSENT_REQUIRED, message)
            .with_data(json!({ "token": token, "plan": plan })));
    }

    let result_plan = plan.clone();
    let job = ctx.subscribe(move |sub| async move {
        let _one_at_a_time = state.build_lock.lock().await;
        let dirs = esp32bench_build::Dirs::new(&state.builds_root, &plan);
        let r = esp32bench_build::run(&plan, &code, &tools, &dirs, |stream, line| {
            sub.send("build.log", json!({ "stream": stream, "line": line }));
        })
        .await;
        let event = match r {
            Ok(out) => json!({ "ok": true, "image": out.image, "elf": out.elf }),
            Err(e) => json!({ "ok": false, "error": e.to_string() }),
        };
        sub.send("build.finished", event);
    });
    Ok(json!({ "job": job, "plan": result_plan }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SerialOpenParams {
    path: PathBuf,
    baud: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SerialWriteParams {
    subscription: u64,
    data: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SerialRefParams {
    subscription: u64,
}

fn serial_error(e: esp32bench_serial::Error) -> RpcError {
    use esp32bench_serial::Error as E;
    let code = match &e {
        E::Baud(_) => code::INVALID_PARAMS,
        E::Busy(_) => code::BUSY,
        E::NotFound(_) => code::NOT_FOUND,
        E::Permission(_) | E::Port { .. } => code::FAILED,
    };
    RpcError::new(code, e.to_string())
}

async fn serial_open(params: Value, ctx: Ctx, state: Arc<State>) -> Result<Value, RpcError> {
    let p: SerialOpenParams = parse_params(params)?;
    // The path is untrusted: only open something device.list reports as a serial
    // port, never an arbitrary file or device node.
    let allowed = match &state.serial_allow {
        Some(list) => list.contains(&p.path),
        None => list_ports(true)
            .await?
            .iter()
            .any(|x| Path::new(&x.path) == p.path),
    };
    if !allowed {
        return Err(RpcError::new(
            code::NOT_FOUND,
            format!(
                "{} is not a serial port this machine reports",
                p.path.display()
            ),
        ));
    }
    let (port, mut rx) = {
        let path = p.path.clone();
        tokio::task::spawn_blocking(move || esp32bench_serial::Port::open(&path, p.baud))
            .await
            .map_err(|e| RpcError::internal(e.to_string()))?
            .map_err(serial_error)?
    };
    let port: SharedPort = Arc::new(std::sync::Mutex::new(port));
    let conn = ctx.connection_id();
    let id = ctx.subscribe(move |sub| {
        // Registered before the task can run, and removed by `entry` when the task
        // ends or is aborted, so the table never holds a closed subscription's port.
        let key = (conn, sub.id());
        state
            .serial_ports
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(key, port);
        let entry = PortEntry {
            state: state.clone(),
            key,
        };
        async move {
            let _entry = entry;
            use base64::Engine;
            while let Some(event) = rx.recv().await {
                match event {
                    esp32bench_serial::Event::Data(bytes) => {
                        let sent = sub.send(
                            "serial.data",
                            json!({
                                "data": base64::engine::general_purpose::STANDARD.encode(&bytes),
                                "text": String::from_utf8_lossy(&bytes),
                            }),
                        );
                        if !sent {
                            break;
                        }
                    }
                    esp32bench_serial::Event::Closed(reason) => {
                        sub.send("serial.closed", json!({ "reason": reason }));
                        break;
                    }
                }
            }
        }
    });
    Ok(json!({ "subscription": id }))
}

/// The port a connection opened under `subscription`.
fn open_port(state: &State, ctx: &Ctx, subscription: u64) -> Result<SharedPort, RpcError> {
    state
        .serial_ports
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(&(ctx.connection_id(), subscription))
        .cloned()
        .ok_or_else(|| {
            RpcError::new(
                code::NOT_FOUND,
                format!("no open serial port {subscription} on this connection"),
            )
        })
}

async fn serial_write(params: Value, ctx: Ctx, state: Arc<State>) -> Result<Value, RpcError> {
    use base64::Engine;
    let p: SerialWriteParams = parse_params(params)?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&p.data)
        .map_err(|e| RpcError::invalid_params(format!("data is not base64: {e}")))?;
    let port = open_port(&state, &ctx, p.subscription)?;
    tokio::task::spawn_blocking(move || {
        port.lock().unwrap_or_else(|p| p.into_inner()).write(&bytes)
    })
    .await
    .map_err(|e| RpcError::internal(e.to_string()))?
    .map_err(serial_error)?;
    Ok(json!({}))
}

async fn serial_reset(params: Value, ctx: Ctx, state: Arc<State>) -> Result<Value, RpcError> {
    let p: SerialRefParams = parse_params(params)?;
    let port = open_port(&state, &ctx, p.subscription)?;
    // The reset pulse sleeps for 100 ms; keep it off the async workers.
    tokio::task::spawn_blocking(move || port.lock().unwrap_or_else(|p| p.into_inner()).reset())
        .await
        .map_err(|e| RpcError::internal(e.to_string()))?
        .map_err(serial_error)?;
    Ok(json!({}))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeviceListParams {
    #[serde(default)]
    all: bool,
}

/// List ports off the async workers; udev enumeration blocks.
async fn list_ports(all: bool) -> Result<Vec<esp32bench_device::Port>, RpcError> {
    tokio::task::spawn_blocking(move || esp32bench_device::list(all))
        .await
        .map_err(|e| RpcError::internal(e.to_string()))?
        .map_err(|e| RpcError::new(code::FAILED, e.to_string()))
}

async fn device_list(params: Value) -> Result<Value, RpcError> {
    let p: DeviceListParams = parse_params(params)?;
    Ok(json!({ "ports": list_ports(p.all).await? }))
}

async fn device_watch(params: Value, ctx: Ctx) -> Result<Value, RpcError> {
    let p: DeviceListParams = parse_params(params)?;
    // Start monitoring before listing, so a change in between is not missed.
    let mut monitor = esp32bench_device::Monitor::new()
        .map_err(|e| RpcError::new(code::UNAVAILABLE, e.to_string()))?;
    let ports = list_ports(p.all).await?;
    let mut last = ports.clone();
    let id = ctx.subscribe(move |sub| async move {
        // Events go out only after the monitor's settle delay (300 ms), so in
        // practice the client has the subscription id before the first one arrives.
        while monitor.changed().await.is_ok() {
            let Ok(now) = list_ports(p.all).await else {
                continue;
            };
            if now == last {
                continue;
            }
            if !sub.send("device.changed", json!({ "ports": now })) {
                break;
            }
            last = now;
        }
    });
    Ok(json!({ "subscription": id, "ports": ports }))
}

/// Holds the daemon's lock file; the lock is released when this is dropped.
pub struct InstanceLock {
    _file: File,
    socket: PathBuf,
}

impl Drop for InstanceLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.socket);
    }
}

/// Take the single-instance lock next to `socket`, then bind the socket (mode 0600).
/// Fails if another daemon holds the lock. A stale socket left by a crashed daemon is
/// replaced, which is safe because the lock proves nobody is serving it.
pub fn bind(socket: &Path) -> io::Result<(UnixListener, InstanceLock)> {
    let dir = socket.parent().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "socket path has no directory")
    })?;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;

    let lock_path = dir.join("core.lock");
    let file = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)?;
    // SAFETY: flock on a file descriptor we own; non-blocking.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(io::Error::new(
            io::ErrorKind::AddrInUse,
            format!(
                "another esp32benchd is running ({} is locked)",
                lock_path.display()
            ),
        ));
    }

    match std::fs::remove_file(socket) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let listener = UnixListener::bind(socket)?;
    std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o600))?;
    Ok((
        listener,
        InstanceLock {
            _file: file,
            socket: socket.to_owned(),
        },
    ))
}
