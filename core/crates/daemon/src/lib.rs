//! The core daemon's method table and socket setup. `main.rs` only parses arguments
//! and handles signals; everything here is testable without a running daemon.

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
pub const PROTOCOL: &str = "0.3.0";
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
}

impl State {
    pub fn new(library: esp32bench_library::Cache, library_source: String) -> Self {
        State {
            library,
            library_source,
            refresh_lock: tokio::sync::Mutex::new(()),
        }
    }

    /// Cache in `$XDG_CACHE_HOME/esp32bench`; source from `$ESP32BENCH_MANIFEST` or the site.
    pub fn from_env() -> io::Result<Self> {
        let dir = esp32bench_library::cache_dir()
            .map_err(|e| io::Error::new(io::ErrorKind::NotFound, e.to_string()))?;
        Ok(Self::new(
            esp32bench_library::Cache::new(dir),
            esp32bench_library::default_source(),
        ))
    }
}

/// Every method the daemon serves. A test checks this against `schema/rpc/`.
pub fn router(state: Arc<State>) -> Router {
    let (s1, s2) = (state.clone(), state);
    Router::new()
        .method("core.hello", |p, _| async move { core_hello(p) })
        .method("job.cancel", |p, ctx| async move { job_cancel(p, ctx) })
        .method("library.list", move |p, _| {
            let s = s1.clone();
            async move { library_list(p, s).await }
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
