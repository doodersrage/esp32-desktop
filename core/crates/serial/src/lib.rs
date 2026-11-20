//! An open serial port: bytes in, bytes out, and a board reset.
//!
//! `serialport` is blocking, so each open port has a reader thread that hands chunks
//! to async code over a channel. Dropping the [`Port`] stops that thread within a
//! read timeout. Output is bytes; callers decode at the edge.

pub mod plot;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio::sync::mpsc;

/// How long a blocking read waits before checking whether the port was closed.
const READ_TIMEOUT: Duration = Duration::from_millis(100);
/// How long RTS is held to reset the chip (esptool's hard reset uses 100 ms).
const RESET_PULSE: Duration = Duration::from_millis(100);
const CHUNK: usize = 4096;

/// Baud rates accepted from callers.
pub const BAUD_RATES: [u32; 10] = [
    9_600, 19_200, 38_400, 57_600, 74_880, 115_200, 230_400, 460_800, 921_600, 2_000_000,
];

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0} baud is not supported")]
    Baud(u32),
    #[error("{0} is in use by another program")]
    Busy(PathBuf),
    #[error(
        "no permission to open {0}; install the esp32bench udev rule or join the group that owns serial ports (uucp on Arch, dialout on Debian, Ubuntu and Fedora)"
    )]
    Permission(PathBuf),
    #[error("{0} does not exist")]
    NotFound(PathBuf),
    #[error("serial port error on {path}: {detail}")]
    Port { path: PathBuf, detail: String },
}

/// What the reader thread reports.
#[derive(Debug)]
pub enum Event {
    Data(Vec<u8>),
    /// The port went away (unplugged) or failed; no more events follow.
    Closed(String),
}

/// An open port. Events arrive on the receiver from [`Port::open`].
pub struct Port {
    path: PathBuf,
    writer: Box<dyn serialport::SerialPort>,
    stop: Arc<AtomicBool>,
}

impl Port {
    /// Open `path` at `baud`, with RTS and DTR off so the board is not held in reset or
    /// in download mode.
    pub fn open(path: &Path, baud: u32) -> Result<(Self, mpsc::UnboundedReceiver<Event>), Error> {
        if !BAUD_RATES.contains(&baud) {
            return Err(Error::Baud(baud));
        }
        let mut port = serialport::new(path.to_string_lossy(), baud)
            .timeout(READ_TIMEOUT)
            .open()
            .map_err(|e| open_error(path, e))?;
        // Best effort: pseudo-terminals and some adapters have no modem lines.
        let _ = port.write_data_terminal_ready(false);
        let _ = port.write_request_to_send(false);

        let reader = port.try_clone().map_err(|e| open_error(path, e))?;
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::unbounded_channel();
        let thread_stop = stop.clone();
        std::thread::Builder::new()
            .name("serial-reader".into())
            .spawn(move || read_loop(reader, tx, thread_stop))
            .map_err(|e| Error::Port {
                path: path.to_owned(),
                detail: e.to_string(),
            })?;
        Ok((
            Port {
                path: path.to_owned(),
                writer: port,
                stop,
            },
            rx,
        ))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Hand `bytes` to the kernel. Deliberately no flush (tcdrain): it waits until the
    /// device has taken every byte, which never happens if the firmware does not read
    /// this port (a C3 reading UART0 while we write to its USB port). The write itself
    /// gives up after the port's 100 ms timeout if the buffer stays full.
    pub fn write(&mut self, bytes: &[u8]) -> Result<(), Error> {
        use std::io::Write;
        self.writer.write_all(bytes).map_err(|e| Error::Port {
            path: self.path.clone(),
            detail: e.to_string(),
        })
    }

    /// Reset the chip the way esptool's hard reset does: DTR off (so it boots the
    /// app, not the ROM downloader), then pulse RTS, which drives EN low.
    /// Blocks for the pulse; call it off the async workers.
    pub fn reset(&mut self) -> Result<(), Error> {
        let err = |e: serialport::Error| Error::Port {
            path: self.path.clone(),
            detail: e.to_string(),
        };
        self.writer.write_data_terminal_ready(false).map_err(err)?;
        self.writer.write_request_to_send(true).map_err(err)?;
        std::thread::sleep(RESET_PULSE);
        self.writer.write_request_to_send(false).map_err(err)?;
        Ok(())
    }
}

impl Drop for Port {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // Throw away output the device never took. Otherwise closing the port waits
        // for it to drain (Linux's closing_wait, 30 s by default), which never happens
        // when the firmware does not read this port, e.g. a C3 reading UART0 while we
        // wrote to its USB port. Seen on hardware; see docs/hardware-notes.md.
        let _ = self.writer.clear(serialport::ClearBuffer::Output);
    }
}

fn open_error(path: &Path, e: serialport::Error) -> Error {
    use serialport::ErrorKind as K;
    let p = path.to_owned();
    match e.kind() {
        // serialport reports EBUSY (another program holds the port exclusively) as
        // NoDevice; a missing device comes back as Io(NotFound) from ENOENT.
        K::NoDevice => Error::Busy(p),
        K::Io(std::io::ErrorKind::PermissionDenied) => Error::Permission(p),
        K::Io(std::io::ErrorKind::NotFound) => Error::NotFound(p),
        K::Io(std::io::ErrorKind::ResourceBusy) => Error::Busy(p),
        _ if e.description.contains("busy") || e.description.contains("Busy") => Error::Busy(p),
        _ => Error::Port {
            path: p,
            detail: e.to_string(),
        },
    }
}

fn read_loop(
    mut port: Box<dyn serialport::SerialPort>,
    tx: mpsc::UnboundedSender<Event>,
    stop: Arc<AtomicBool>,
) {
    let mut buf = vec![0u8; CHUNK];
    while !stop.load(Ordering::Relaxed) && !tx.is_closed() {
        match port.read(&mut buf) {
            Ok(0) => {
                // A tty returns 0 when the other end has gone (unplugged, or a pty closed).
                let _ = tx.send(Event::Closed("the port was closed".into()));
                return;
            }
            Ok(n) => {
                if tx.send(Event::Data(buf[..n].to_vec())).is_err() {
                    return;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::TimedOut => continue,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => {
                let _ = tx.send(Event::Closed(e.to_string()));
                return;
            }
        }
    }
}
