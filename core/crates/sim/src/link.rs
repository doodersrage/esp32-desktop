//! The simulator's local links to QEMU (UART, QMP, the C3's USB serial port) and to
//! `esp32bench-qemu-host` (the pin link): Unix sockets in the session's private
//! directory, or on Windows loopback TCP ports (QEMU's `tcp:` chardevs). Loopback is
//! reachable by other users of the same computer, which a Unix socket in a 0700
//! directory is not; named pipes would close that gap (docs/platforms.md).

use std::ffi::OsString;
use std::io;
use std::path::Path;

#[cfg(unix)]
pub type Stream = tokio::net::UnixStream;
#[cfg(unix)]
pub use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};

#[cfg(windows)]
pub type Stream = tokio::net::TcpStream;
#[cfg(windows)]
pub use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};

/// Where one link's server listens.
#[cfg(unix)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint(std::path::PathBuf);

#[cfg(windows)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint(std::net::SocketAddr);

impl Endpoint {
    /// A new endpoint named `name` for a session whose directory is `dir`: the socket
    /// path there, or on Windows a free loopback port.
    #[cfg(unix)]
    pub fn new(dir: &Path, name: &str) -> io::Result<Self> {
        Ok(Endpoint(dir.join(name)))
    }

    #[cfg(windows)]
    pub fn new(_dir: &Path, _name: &str) -> io::Result<Self> {
        // Let the system pick a free port, then hand it to QEMU or the host. Another
        // program could take it in between; QEMU then fails to start and says so.
        let l = std::net::TcpListener::bind(("127.0.0.1", 0))?;
        Ok(Endpoint(l.local_addr()?))
    }

    /// The QEMU chardev for a server here, without its options:
    /// `unix:<path>` or `tcp:127.0.0.1:<port>`.
    pub fn chardev(&self) -> OsString {
        #[cfg(unix)]
        {
            let mut s = OsString::from("unix:");
            s.push(&self.0);
            s
        }
        #[cfg(windows)]
        {
            OsString::from(format!("tcp:{}", self.0))
        }
    }

    /// The endpoint as `esp32bench-qemu-host` takes it (`ESP32BENCH_GPIO_SOCK`): the
    /// socket's path, or `127.0.0.1:<port>`.
    pub fn as_env(&self) -> OsString {
        #[cfg(unix)]
        {
            self.0.clone().into_os_string()
        }
        #[cfg(windows)]
        {
            OsString::from(self.0.to_string())
        }
    }

    pub async fn connect(&self) -> io::Result<Stream> {
        Stream::connect(&self.0).await
    }

    /// Whether the server is not there yet (keep waiting) rather than broken.
    pub fn not_yet(e: &io::Error) -> bool {
        matches!(
            e.kind(),
            io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
        )
    }
}

impl std::fmt::Display for Endpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        #[cfg(unix)]
        {
            write!(f, "{}", self.0.display())
        }
        #[cfg(windows)]
        {
            write!(f, "{}", self.0)
        }
    }
}
