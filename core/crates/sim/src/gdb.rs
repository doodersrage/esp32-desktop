//! A GDB remote-serial-protocol probe. It checks that QEMU's GDB stub answers,
//! without needing the chip's toolchain GDB. The full GDB/MI client comes later.

use std::net::SocketAddr;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::Error;

/// Frame a payload as an RSP packet: `$payload#checksum`.
pub fn packet(payload: &str) -> String {
    let sum = payload.bytes().fold(0u8, |a, b| a.wrapping_add(b));
    format!("${payload}#{sum:02x}")
}

/// Ask the stub why the target halted (`?`) and return the stop reply, e.g. `T05thread:01;`.
pub async fn probe(addr: SocketAddr, timeout: Duration) -> Result<String, Error> {
    tokio::time::timeout(timeout, probe_inner(addr))
        .await
        .map_err(|_| Error::Timeout("GDB stub reply"))?
}

async fn probe_inner(addr: SocketAddr) -> Result<String, Error> {
    let mut s = TcpStream::connect(addr).await?;
    s.write_all(packet("?").as_bytes()).await?;

    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        if s.read(&mut byte).await? == 0 {
            return Err(Error::GdbProtocol("stub closed the connection".into()));
        }
        buf.push(byte[0]);
        // Skip acks (`+`) until a packet starts, then read to `#xx`.
        if let Some(start) = buf.iter().position(|b| *b == b'$')
            && let Some(hash) = buf[start..].iter().position(|b| *b == b'#')
            && buf.len() >= start + hash + 3
        {
            let payload = &buf[start + 1..start + hash];
            let reply = String::from_utf8_lossy(payload).into_owned();
            let _ = s.write_all(b"+").await;
            return Ok(reply);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    #[test]
    fn frames_packets() {
        assert_eq!(packet("?"), "$?#3f");
        assert_eq!(packet("g"), "$g#67");
    }

    #[tokio::test]
    async fn reads_stop_reply() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            let mut req = [0u8; 5];
            s.read_exact(&mut req).await.unwrap();
            assert_eq!(&req, b"$?#3f");
            s.write_all(b"+$T05thread:01;#07").await.unwrap();
            let mut ack = [0u8; 1];
            s.read_exact(&mut ack).await.unwrap();
            assert_eq!(&ack, b"+");
        });
        let reply = probe(addr, Duration::from_secs(2)).await.unwrap();
        assert_eq!(reply, "T05thread:01;");
        server.await.unwrap();
    }
}
