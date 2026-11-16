//! Port against a pseudo-terminal pair: the test holds the "board" end (the pty
//! master) and Port opens the other end like a serial device.

use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::PathBuf;
use std::time::Duration;

use esp32bench_serial::{Error, Event, Port};

/// A pty pair: the master fd (the "board") and the path of the slave (the "port").
fn pty() -> (OwnedFd, PathBuf) {
    // posix_openpt with O_CLOEXEC, not openpty: openpty's descriptors are inheritable
    // until a later fcntl, and a QEMU spawned by a parallel test in that window keeps
    // the master open, so dropping it here would no longer hang up the port. The
    // slave is never opened here; the code under test opens it by path.
    // SAFETY: plain libc calls on the fd posix_openpt returns, with a large enough
    // name buffer for ptsname_r.
    let master = unsafe { libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC) };
    assert!(master >= 0, "posix_openpt failed");
    let mut name = [0 as libc::c_char; 128];
    unsafe {
        assert_eq!(libc::grantpt(master), 0);
        assert_eq!(libc::unlockpt(master), 0);
        assert_eq!(libc::ptsname_r(master, name.as_mut_ptr(), name.len()), 0);
    }
    let path = unsafe { std::ffi::CStr::from_ptr(name.as_ptr()) }
        .to_string_lossy()
        .into_owned();
    (unsafe { OwnedFd::from_raw_fd(master) }, PathBuf::from(path))
}

fn board_write(master: &OwnedFd, bytes: &[u8]) {
    // SAFETY: write to a valid fd from a valid buffer.
    let n = unsafe { libc::write(master.as_raw_fd(), bytes.as_ptr().cast(), bytes.len()) };
    assert_eq!(n, bytes.len() as isize);
}

/// Read what the port wrote to the board end, waiting up to a second.
fn board_read(master: &OwnedFd) -> Vec<u8> {
    let mut pfd = libc::pollfd {
        fd: master.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: poll on one valid pollfd.
    assert!(
        unsafe { libc::poll(&mut pfd, 1, 1000) } > 0,
        "nothing written to the board"
    );
    let mut buf = [0u8; 256];
    // SAFETY: read into a valid buffer.
    let n = unsafe { libc::read(master.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
    buf[..n as usize].to_vec()
}

async fn next(rx: &mut tokio::sync::mpsc::UnboundedReceiver<Event>) -> Event {
    tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn bytes_flow_both_ways_and_stay_bytes() {
    let (master, path) = pty();
    let (mut port, mut rx) = Port::open(&path, 115_200).unwrap();

    // Not valid UTF-8 on purpose: the port must pass bytes through untouched.
    board_write(&master, b"rst:0x1\xff\xfe\n");
    let mut got = Vec::new();
    while got.len() < 10 {
        match next(&mut rx).await {
            Event::Data(d) => got.extend(d),
            Event::Closed(r) => panic!("closed early: {r}"),
        }
    }
    assert_eq!(got, b"rst:0x1\xff\xfe\n");

    port.write(b"SET wifi_ssid x\n").unwrap();
    assert_eq!(board_read(&master), b"SET wifi_ssid x\n");
}

#[tokio::test]
async fn reports_when_the_board_goes_away() {
    let (master, path) = pty();
    let (_port, mut rx) = Port::open(&path, 115_200).unwrap();
    drop(master); // like unplugging the board
    loop {
        match next(&mut rx).await {
            Event::Closed(_) => break,
            Event::Data(_) => continue,
        }
    }
}

#[tokio::test]
async fn dropping_the_port_stops_its_reader_thread() {
    let (_master, path) = pty();
    let (port, mut rx) = Port::open(&path, 115_200).unwrap();
    drop(port);
    // The reader thread holds the only sender; the channel ends when the thread does.
    let ended = tokio::time::timeout(Duration::from_secs(2), async {
        while rx.recv().await.is_some() {}
    })
    .await;
    assert!(
        ended.is_ok(),
        "serial reader thread still running after drop"
    );
}

#[test]
fn reports_a_port_held_by_someone_else_as_busy() {
    let (_master, path) = pty();
    let (_first, _rx) = Port::open(&path, 115_200).unwrap();
    assert!(matches!(Port::open(&path, 115_200), Err(Error::Busy(_))));
}

#[test]
fn refuses_bad_baud_and_missing_ports() {
    let (_master, path) = pty();
    assert!(matches!(
        Port::open(&path, 12_345),
        Err(Error::Baud(12_345))
    ));
    assert!(matches!(
        Port::open(std::path::Path::new("/dev/ttyESP32BENCH_NOPE"), 115_200),
        Err(Error::NotFound(_))
    ));
}

#[test]
fn writing_to_a_port_nobody_reads_returns_promptly() {
    // A port nobody reads must not block a write. (A pty does not reproduce the
    // original hang: its drain returns at once. The real case, a C3 whose firmware
    // reads UART0 while we write to its USB port, was checked on hardware; see
    // docs/hardware-notes.md.)
    let (_master, path) = pty();
    let (mut port, _rx) = Port::open(&path, 115_200).unwrap();
    let start = std::time::Instant::now();
    let _ = port.write(b"ping 42\n");
    assert!(
        start.elapsed() < Duration::from_secs(1),
        "write blocked for {:?}",
        start.elapsed()
    );
}
