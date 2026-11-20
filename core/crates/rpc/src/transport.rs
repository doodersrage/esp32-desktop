//! The local stream the UI and the CLI reach the core over: a Unix socket at a path,
//! or on Windows a named pipe. Callers deal in paths either way; on Windows a path is
//! turned into a pipe name ([`pipe_name`]), which is also what Qt's `QLocalSocket`
//! connects to there.

use std::io;
use std::path::Path;

use tokio::io::{AsyncRead, AsyncWrite};

/// What a connection is read and written through.
pub trait Stream: AsyncRead + AsyncWrite + Send + Unpin + 'static {}
impl<T: AsyncRead + AsyncWrite + Send + Unpin + 'static> Stream for T {}

#[cfg(unix)]
mod imp {
    use super::*;
    use tokio::net::{UnixListener, UnixStream};

    pub struct Listener(UnixListener);

    impl Listener {
        /// Listen at `path`, which must not exist (the daemon removes a stale socket
        /// first, under its lock).
        pub fn bind(path: &Path) -> io::Result<Self> {
            UnixListener::bind(path).map(Listener)
        }

        pub async fn accept(&mut self) -> io::Result<UnixStream> {
            self.0.accept().await.map(|(s, _)| s)
        }
    }

    pub async fn connect(path: &Path) -> io::Result<UnixStream> {
        UnixStream::connect(path).await
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use std::ffi::OsString;
    use tokio::net::windows::named_pipe::{
        ClientOptions, NamedPipeClient, NamedPipeServer, ServerOptions,
    };

    /// Accepts connections on a named pipe. Each connection takes the waiting pipe
    /// instance, and a new one is created for the next client.
    pub struct Listener {
        name: OsString,
        next: NamedPipeServer,
    }

    impl Listener {
        /// Create the pipe for `path`. It fails if the pipe exists, so a second core
        /// cannot take over the first one's name: on Windows this is the
        /// single-instance check the lock file is on Unix. Remote clients are refused.
        pub fn bind(path: &Path) -> io::Result<Self> {
            let name = pipe_name(path);
            let next = ServerOptions::new()
                .first_pipe_instance(true)
                .reject_remote_clients(true)
                .create(&name)?;
            Ok(Listener { name, next })
        }

        pub async fn accept(&mut self) -> io::Result<NamedPipeServer> {
            self.next.connect().await?;
            let fresh = ServerOptions::new()
                .reject_remote_clients(true)
                .create(&self.name)?;
            Ok(std::mem::replace(&mut self.next, fresh))
        }
    }

    pub async fn connect(path: &Path) -> io::Result<NamedPipeClient> {
        // ERROR_PIPE_BUSY: every instance is taken; the server makes another as soon
        // as it accepts, so wait briefly and try again.
        const ERROR_PIPE_BUSY: i32 = 231;
        let name = pipe_name(path);
        for _ in 0..50 {
            match ClientOptions::new().open(&name) {
                Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) => {
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }
                r => return r,
            }
        }
        ClientOptions::new().open(&name)
    }
}

pub use imp::{Listener, connect};

/// The named pipe for `path`: the path itself if it is one (`\\.\pipe\...`), else a
/// name made from it, so paths keep working as endpoint names on Windows (tests bind
/// sockets in temporary directories). Pipe names may not contain `\` after the prefix.
pub fn pipe_name(path: &Path) -> std::ffi::OsString {
    const PREFIX: &str = r"\\.\pipe\";
    let s = path.to_string_lossy();
    if s.starts_with(PREFIX) {
        return path.as_os_str().to_owned();
    }
    let flat: String = s
        .chars()
        .map(|c| {
            if matches!(c, '\\' | '/' | ':') {
                '-'
            } else {
                c
            }
        })
        .collect();
    format!("{PREFIX}esp32bench{flat}").into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_become_pipe_names() {
        assert_eq!(
            pipe_name(Path::new(r"\\.\pipe\esp32bench-ana-core")),
            r"\\.\pipe\esp32bench-ana-core"
        );
        assert_eq!(
            pipe_name(Path::new(r"C:\Users\ana\AppData\Local\Temp\x\core.sock")),
            r"\\.\pipe\esp32benchC--Users-ana-AppData-Local-Temp-x-core.sock"
        );
        assert_eq!(
            pipe_name(Path::new("/tmp/x/core.sock")),
            r"\\.\pipe\esp32bench-tmp-x-core.sock"
        );
    }
}
