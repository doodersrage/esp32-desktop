//! The core daemon's method table and socket setup. `main.rs` only parses arguments
//! and handles signals; everything here is testable without a running daemon.

use std::fs::File;
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};

use esp32bench_rpc::{Router, RpcError, code, parse_params};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::net::UnixListener;

/// Protocol version, as in `schema/rpc/index.json`. A test keeps them equal.
pub const PROTOCOL: &str = "0.1.0";
pub const NAME: &str = "esp32benchd";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Every method the daemon serves. A test checks this against `schema/rpc/`.
pub fn router() -> Router {
    Router::new()
        .method("core.hello", |p, _| async move { core_hello(p) })
        .method("device.list", |p, _| async move { device_list(p).await })
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
struct DeviceListParams {
    #[serde(default)]
    all: bool,
}

async fn device_list(params: Value) -> Result<Value, RpcError> {
    let p: DeviceListParams = parse_params(params)?;
    // udev enumeration blocks; keep it off the async workers.
    let ports = tokio::task::spawn_blocking(move || esp32bench_device::list(p.all))
        .await
        .map_err(|e| RpcError::internal(e.to_string()))?
        .map_err(|e| RpcError::new(code::FAILED, e.to_string()))?;
    Ok(json!({ "ports": ports }))
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
