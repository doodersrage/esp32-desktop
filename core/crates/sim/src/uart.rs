//! The console: UART0 over QEMU's socket chardev, and on the C3 on lcgamboa's fork also
//! its USB-Serial-JTAG serial port (boards like the C3 SuperMini have no USB-UART chip,
//! so their `Serial` is that). Output is raw bytes; callers decode at the edge.

use std::collections::VecDeque;
use std::time::Duration;

use crate::link::Stream;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::Instant;

use crate::Error;

/// Banners that mean the firmware crashed.
pub const PANIC_MARKERS: [&[u8]; 3] = [
    b"Guru Meditation Error",
    b"abort() was called",
    b"assert failed: ", // IDF's assert handler, e.g. a failed flash init at start-up
];

/// Lines that end a panic report.
const PANIC_END_MARKERS: [&[u8]; 2] = [b"Rebooting...", b"CPU halted."];

/// How long to keep reading after a panic banner to capture the register dump and backtrace.
const PANIC_GRACE: Duration = Duration::from_secs(2);

/// Bytes kept for marker matching while waiting. Older output is still passed to the sink.
const WINDOW: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WaitOutcome {
    Found,
    /// The panic report, from the banner to `Rebooting...` (or as much as arrived in time),
    /// decoded lossily. The raw bytes went to the sink.
    Panic(String),
    Timeout,
    Closed,
}

/// A line one port printed counts as the same as the other's within this time.
const SAME_LINE: Duration = Duration::from_secs(2);

/// How long a merged console holds an unfinished line (a prompt) before passing it on.
const HOLD: Duration = Duration::from_millis(50);

/// UART0 and the USB serial port merged into one console. The ROM writes its boot
/// text to both, so merging raw chunks interleaved the two copies mid-line: each
/// port's output is taken a whole line at a time, and a line the other port printed
/// just before is dropped, so the console reads as a real board's USB port does.
#[derive(Default)]
struct Merge {
    partial: [Vec<u8>; 2],
    recent: VecDeque<(usize, Vec<u8>, Instant)>,
}

impl Merge {
    /// `port`'s new bytes: what to show now.
    fn push(&mut self, port: usize, bytes: &[u8], now: Instant) -> Vec<u8> {
        self.partial[port].extend_from_slice(bytes);
        let mut out = Vec::new();
        while let Some(end) = self.partial[port].iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.partial[port].drain(..=end).collect();
            self.line(port, line, now, &mut out);
        }
        out
    }

    /// The unfinished lines, after a pause in the output.
    fn flush(&mut self, now: Instant) -> Vec<u8> {
        let mut out = Vec::new();
        for port in 0..2 {
            let line = std::mem::take(&mut self.partial[port]);
            if !line.is_empty() {
                self.line(port, line, now, &mut out);
            }
        }
        out
    }

    fn pending(&self) -> bool {
        self.partial.iter().any(|p| !p.is_empty())
    }

    fn line(&mut self, port: usize, line: Vec<u8>, now: Instant, out: &mut Vec<u8>) {
        while self
            .recent
            .front()
            .is_some_and(|(_, _, at)| now.duration_since(*at) > SAME_LINE)
        {
            self.recent.pop_front();
        }
        let key: Vec<u8> = {
            let mut k = line.as_slice();
            while let [rest @ .., b'\r' | b'\n'] = k {
                k = rest;
            }
            k.to_vec()
        };
        // Blank lines are too common to be the same line.
        if !key.is_empty() {
            if let Some(i) = self
                .recent
                .iter()
                .position(|(p, k, _)| *p != port && *k == key)
            {
                self.recent.remove(i);
                return;
            }
            self.recent.push_back((port, key, now));
            if self.recent.len() > 256 {
                self.recent.pop_front();
            }
        }
        out.extend_from_slice(&line);
    }
}

/// The console's sockets are read all the time, by a task per port, into a queue
/// that `read_chunk` takes from. QEMU waits for the socket to have room rather than
/// dropping output (patch 0017), so a caller busy with something else (the pins,
/// say) must not leave the socket full: that would stop the firmware.
pub struct Uart {
    chunks: tokio::sync::mpsc::UnboundedReceiver<Chunk>,
    queue: tokio::sync::mpsc::UnboundedSender<Chunk>,
    writer: tokio::io::WriteHalf<Stream>,
    /// The USB-Serial-JTAG serial port, when the machine has one wired.
    usb: Option<tokio::io::WriteHalf<Stream>>,
    readers: Vec<tokio::task::JoinHandle<()>>,
    closed: bool,
    /// Set when the USB port is merged in.
    merge: Option<Merge>,
}

/// What a port's reader task queues: output (from port 0, UART0, or 1, USB), or
/// UART0's end.
enum Chunk {
    Data(usize, Vec<u8>),
    Closed,
}

/// Read `port` until it ends, queueing what it says. Only UART0's end closes the
/// console; the USB port ending leaves UART0 going.
fn spawn_reader(
    mut port: tokio::io::ReadHalf<Stream>,
    queue: tokio::sync::mpsc::UnboundedSender<Chunk>,
    port_index: usize,
) -> tokio::task::JoinHandle<()> {
    let uart0 = port_index == 0;
    tokio::spawn(async move {
        let mut buf = vec![0u8; 4096];
        loop {
            match port.read(&mut buf).await {
                Ok(n) if n > 0 => {
                    if queue
                        .send(Chunk::Data(port_index, buf[..n].to_vec()))
                        .is_err()
                    {
                        return;
                    }
                }
                _ => {
                    if uart0 {
                        let _ = queue.send(Chunk::Closed);
                    }
                    return;
                }
            }
        }
    })
}

impl Uart {
    pub fn new(stream: Stream) -> Self {
        let (read, writer) = tokio::io::split(stream);
        let (queue, chunks) = tokio::sync::mpsc::unbounded_channel();
        let reader = spawn_reader(read, queue.clone(), 0);
        Uart {
            chunks,
            queue,
            writer,
            usb: None,
            readers: vec![reader],
            closed: false,
            merge: None,
        }
    }

    /// Also read from (and write to) the chip's USB serial port.
    pub fn with_usb(mut self, usb: Stream) -> Self {
        let (read, writer) = tokio::io::split(usb);
        self.readers.push(spawn_reader(read, self.queue.clone(), 1));
        self.usb = Some(writer);
        self.merge = Some(Merge::default());
        self
    }

    /// Send bytes to the firmware: UART0's RX, and the USB serial port's OUT endpoint
    /// when there is one. A sketch reads whichever its `Serial` is.
    pub async fn write(&mut self, bytes: &[u8]) -> Result<(), Error> {
        self.writer.write_all(bytes).await?;
        if let Some(usb) = &mut self.usb
            && usb.write_all(bytes).await.is_err()
        {
            self.usb = None;
        }
        Ok(())
    }

    /// The next chunk, from either port. `None` means QEMU closed UART0's socket.
    pub async fn read_chunk(&mut self) -> Result<Option<Vec<u8>>, Error> {
        if self.closed {
            return Ok(None);
        }
        let Some(merge) = &mut self.merge else {
            return match self.chunks.recv().await {
                Some(Chunk::Data(_, bytes)) => Ok(Some(bytes)),
                Some(Chunk::Closed) | None => {
                    self.closed = true;
                    Ok(None)
                }
            };
        };
        loop {
            let next = if merge.pending() {
                match tokio::time::timeout(HOLD, self.chunks.recv()).await {
                    Ok(next) => next,
                    Err(_) => {
                        // Quiet: show the unfinished line, a prompt say.
                        return Ok(Some(merge.flush(Instant::now())));
                    }
                }
            } else {
                self.chunks.recv().await
            };
            match next {
                Some(Chunk::Data(port, bytes)) => {
                    let out = merge.push(port, &bytes, Instant::now());
                    if !out.is_empty() {
                        return Ok(Some(out));
                    }
                }
                Some(Chunk::Closed) | None => {
                    self.closed = true;
                    let rest = merge.flush(Instant::now());
                    return Ok((!rest.is_empty()).then_some(rest));
                }
            }
        }
    }

    /// Read until `marker` appears, a panic banner appears, the socket closes, or
    /// `timeout` passes. Every chunk read is handed to `sink` as it arrives.
    pub async fn wait_for(
        &mut self,
        marker: &[u8],
        timeout: Duration,
        mut sink: impl FnMut(&[u8]),
    ) -> Result<WaitOutcome, Error> {
        let deadline = Instant::now() + timeout;
        let mut window: Vec<u8> = Vec::new();
        loop {
            let chunk = match tokio::time::timeout_at(deadline, self.read_chunk()).await {
                Err(_) => return Ok(WaitOutcome::Timeout),
                Ok(r) => r?,
            };
            let Some(chunk) = chunk else {
                return Ok(WaitOutcome::Closed);
            };
            sink(&chunk);
            window.extend_from_slice(&chunk);
            if contains(&window, marker) {
                return Ok(WaitOutcome::Found);
            }
            if let Some(pos) = PANIC_MARKERS.iter().filter_map(|p| find(&window, p)).min() {
                let report = self
                    .read_panic_report(window.split_off(pos), &mut sink)
                    .await?;
                return Ok(WaitOutcome::Panic(
                    String::from_utf8_lossy(&report).into_owned(),
                ));
            }
            if window.len() > WINDOW {
                // Keep a tail long enough to match a marker split across chunks.
                let keep = marker.len().max(32);
                let cut = window.len() - keep;
                window.drain(..cut);
            }
        }
    }

    /// Keep reading after a panic banner until the report ends, QEMU goes quiet or closes.
    async fn read_panic_report(
        &mut self,
        mut report: Vec<u8>,
        sink: &mut impl FnMut(&[u8]),
    ) -> Result<Vec<u8>, Error> {
        let deadline = Instant::now() + PANIC_GRACE;
        loop {
            if let Some(end) = PANIC_END_MARKERS
                .iter()
                .filter_map(|m| find(&report, m).map(|i| i + m.len()))
                .min()
            {
                report.truncate(end);
                return Ok(report);
            }
            match tokio::time::timeout_at(deadline, self.read_chunk()).await {
                Ok(Ok(Some(chunk))) => {
                    sink(&chunk);
                    report.extend_from_slice(&chunk);
                }
                Ok(Err(e)) => return Err(e),
                Ok(Ok(None)) | Err(_) => return Ok(report),
            }
        }
    }
}

impl Drop for Uart {
    fn drop(&mut self) {
        for r in &self.readers {
            r.abort();
        }
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    find(haystack, needle).is_some()
}

// Socket pairs are Unix only; the code under test is the same everywhere.
#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;
    use tokio::net::UnixStream;

    // The socket is drained even while nobody asks for output, so a firmware
    // waiting for room (patch 0017) is never held up, and nothing is lost.
    #[tokio::test]
    async fn drains_the_socket_while_nobody_reads() {
        let (a, mut b) = UnixStream::pair().unwrap();
        let mut uart = Uart::new(a);
        let data: Vec<u8> = (0..1_000_000u32).map(|i| (i % 251) as u8).collect();
        // Far more than a socket holds; it only completes if something drains it.
        tokio::time::timeout(Duration::from_secs(10), b.write_all(&data))
            .await
            .expect("the writer was held up: nothing drained the socket")
            .unwrap();
        drop(b);
        let mut got = Vec::new();
        while let Some(chunk) = uart.read_chunk().await.unwrap() {
            got.extend_from_slice(&chunk);
        }
        assert_eq!(got, data);
        assert_eq!(uart.read_chunk().await.unwrap(), None);
    }

    // The ROM's boot text on both ports reads once, line by line, not interleaved.
    #[test]
    fn merged_ports_show_each_line_once() {
        let now = Instant::now();
        let mut m = Merge::default();
        let mut out = Vec::new();
        out.extend(m.push(0, b"load:0x3fcd5820,len:0x1010\r\nload:0x", now));
        out.extend(m.push(1, b"load:0x3fcd5820,len:0x1010\r\nload:0x403cbf10", now));
        out.extend(m.push(0, b"403cbf10,len:0x9f4\r\n", now));
        out.extend(m.push(1, b",len:0x9f4\r\nHello from Serial\r\n\r\n", now));
        out.extend(m.push(0, b"\r\n", now));
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "load:0x3fcd5820,len:0x1010\r\nload:0x403cbf10,len:0x9f4\r\nHello from Serial\r\n\r\n\r\n"
        );
        // The same line again later is new output, and an unfinished one shows after a pause.
        let later = now + SAME_LINE + Duration::from_millis(1);
        assert_eq!(
            m.push(0, b"Hello from Serial\n", later),
            b"Hello from Serial\n"
        );
        assert!(m.push(1, b"> ", later).is_empty());
        assert!(m.pending());
        assert_eq!(m.flush(later), b"> ");
    }

    #[tokio::test]
    async fn merges_the_usb_port() {
        let (a, mut b) = UnixStream::pair().unwrap();
        let (c, mut d) = UnixStream::pair().unwrap();
        let mut uart = Uart::new(a).with_usb(c);
        b.write_all(b"rst:0x15 (USB_UART_CHIP_RESET)\r\n")
            .await
            .unwrap();
        d.write_all(b"rst:0x15 (USB_UART_CHIP_RESET)\r\nready> ")
            .await
            .unwrap();
        let mut got = Vec::new();
        while !got.ends_with(b"ready> ") {
            got.extend(uart.read_chunk().await.unwrap().unwrap());
        }
        assert_eq!(got, b"rst:0x15 (USB_UART_CHIP_RESET)\r\nready> ");
        drop(b);
        assert_eq!(uart.read_chunk().await.unwrap(), None);
    }

    #[tokio::test]
    async fn finds_marker_split_across_chunks() {
        let (a, mut b) = UnixStream::pair().unwrap();
        let mut uart = Uart::new(a);
        let writer = tokio::spawn(async move {
            b.write_all(b"ets Jun  8 2016\r\nrs").await.unwrap();
            tokio::time::sleep(Duration::from_millis(20)).await;
            b.write_all(b"t:0x1 (POWERON_RESET)\r\n").await.unwrap();
            b
        });
        let mut seen = Vec::new();
        let out = uart
            .wait_for(b"rst:0x1", Duration::from_secs(2), |c| {
                seen.extend_from_slice(c)
            })
            .await
            .unwrap();
        assert_eq!(out, WaitOutcome::Found);
        assert!(seen.starts_with(b"ets Jun"));
        drop(writer.await.unwrap());
    }

    #[tokio::test]
    async fn reports_panic_and_keeps_non_utf8_bytes() {
        let (a, mut b) = UnixStream::pair().unwrap();
        let mut uart = Uart::new(a);
        let writer = tokio::spawn(async move {
            b.write_all(b"\xff\xfe garbage\r\nGuru Meditation Error: Core 0 panic'ed")
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_millis(20)).await;
            b.write_all(b"\r\nBacktrace: 0x4016744b:0x3ffd9200\r\n\r\nRebooting...\r\nets Jul")
                .await
                .unwrap();
            b
        });
        let mut seen = Vec::new();
        let out = uart
            .wait_for(b"app_main", Duration::from_secs(2), |c| {
                seen.extend_from_slice(c)
            })
            .await
            .unwrap();
        let WaitOutcome::Panic(report) = out else {
            panic!("expected panic, got {out:?}");
        };
        assert!(report.starts_with("Guru Meditation Error"));
        assert!(report.contains("Backtrace: 0x4016744b"));
        assert!(report.ends_with("Rebooting..."));
        assert_eq!(&seen[..2], b"\xff\xfe");
        drop(writer.await.unwrap());
    }

    #[tokio::test]
    async fn returns_partial_panic_report_when_output_stops() {
        let (a, mut b) = UnixStream::pair().unwrap();
        let mut uart = Uart::new(a);
        b.write_all(b"abort() was called at PC 0x400d1234")
            .await
            .unwrap();
        drop(b);
        let out = uart
            .wait_for(b"app_main", Duration::from_secs(2), |_| {})
            .await
            .unwrap();
        assert_eq!(
            out,
            WaitOutcome::Panic("abort() was called at PC 0x400d1234".into())
        );
    }

    #[tokio::test]
    async fn treats_idf_assert_as_panic() {
        let (a, mut b) = UnixStream::pair().unwrap();
        let mut uart = Uart::new(a);
        b.write_all(b"entry 0x403cc710\r\n\r\nassert failed: do_core_init startup.c:328 (flash_ret == ESP_OK)\r\nRebooting...\r\n")
            .await
            .unwrap();
        let out = uart
            .wait_for(b"app_main", Duration::from_secs(2), |_| {})
            .await
            .unwrap();
        let WaitOutcome::Panic(report) = out else {
            panic!("expected panic, got {out:?}");
        };
        assert!(report.starts_with("assert failed: do_core_init"));
    }

    #[tokio::test]
    async fn writes_reach_the_peer() {
        let (a, mut b) = UnixStream::pair().unwrap();
        let mut uart = Uart::new(a);
        uart.write(b"ping\r\n").await.unwrap();
        let mut got = [0u8; 6];
        b.read_exact(&mut got).await.unwrap();
        assert_eq!(&got, b"ping\r\n");
    }

    #[tokio::test]
    async fn times_out_and_detects_close() {
        let (a, b) = UnixStream::pair().unwrap();
        let mut uart = Uart::new(a);
        let out = uart
            .wait_for(b"x", Duration::from_millis(50), |_| {})
            .await
            .unwrap();
        assert_eq!(out, WaitOutcome::Timeout);
        drop(b);
        let out = uart
            .wait_for(b"x", Duration::from_secs(2), |_| {})
            .await
            .unwrap();
        assert_eq!(out, WaitOutcome::Closed);
    }
}
