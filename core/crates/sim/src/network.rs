//! The simulated Wi-Fi network: lcgamboa's fork's access point, behind QEMU's
//! user-mode network in restricted mode.
//!
//! A sketch that joins [`SSID`] (open, no password) gets an address by DHCP
//! (10.0.2.15) and nothing else: in restricted mode QEMU answers DHCP but drops every
//! packet bound elsewhere, name lookups included, and the host's own addresses are
//! unreachable too. The one way in is port 80, forwarded from a port on the host's
//! loopback, so a sketch's web page opens in a browser on this computer.
//! `tests/firmware/arduino-wifi` checks all of that (`lcgamboa_*_wifi_*` boot tests).

use std::ffi::OsString;

/// The access point's name. It is the fork's (`hw/misc/esp32_wifi_ap.c`), and open.
pub const SSID: &str = "PICSimLabWifi";

/// A session's network: the host port forwarded to the sketch's port 80.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Network {
    pub web_port: u16,
}

impl Network {
    /// A network whose web port is one the system has free now. Another program could
    /// take it before QEMU binds it; QEMU then fails to start and says so.
    pub fn on_a_free_port() -> std::io::Result<Self> {
        let l = std::net::TcpListener::bind(("127.0.0.1", 0))?;
        Ok(Network {
            web_port: l.local_addr()?.port(),
        })
    }

    /// The sketch's web page, from this computer.
    pub fn web_url(&self) -> String {
        format!("http://127.0.0.1:{}/", self.web_port)
    }

    /// QEMU arguments: the restricted user-mode network on hub 0, where the fork's
    /// radio (the `-nic hubport,hubid=0,model=...` of [`crate::Launch`]) sits.
    pub fn qemu_args(&self) -> Vec<OsString> {
        vec![
            "-netdev".into(),
            format!(
                "user,id=esp32bench-net,restrict=on,ipv6=off,hostfwd=tcp:127.0.0.1:{}-:80",
                self.web_port
            )
            .into(),
            "-netdev".into(),
            "hubport,id=esp32bench-hub,hubid=0,netdev=esp32bench-net".into(),
        ]
    }
}

/// Joins a sketch to the simulated network through the esp32bench.dev settings block
/// that library sketches carry: at start they print `SETTINGS<tab>key=set|unset...`
/// and, for 2.5 s, take `SET<tab>key<tab>value` lines and `SAVE` (which stores them and
/// restarts). Fed a session's output, [`Joiner::feed`] returns the lines to send, once:
/// when the sketch reports a `wifi_ssid` setting that is not set. A sketch whose Wi-Fi
/// is already set, or that has no settings block, is left alone.
#[derive(Debug, Default)]
pub struct Joiner {
    line: Vec<u8>,
    done: bool,
}

impl Joiner {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the lines were sent (returned) already.
    pub fn done(&self) -> bool {
        self.done
    }

    pub fn feed(&mut self, bytes: &[u8]) -> Option<Vec<u8>> {
        if self.done {
            return None;
        }
        for &b in bytes {
            if b != b'\n' {
                if self.line.len() < 512 {
                    self.line.push(b);
                }
                continue;
            }
            let line = std::mem::take(&mut self.line);
            let line = line.strip_suffix(b"\r").unwrap_or(&line);
            if line.starts_with(b"SETTINGS")
                && line.split(|&c| c == b'\t').any(|f| f == b"wifi_ssid=unset")
            {
                self.done = true;
                return Some(join_lines());
            }
        }
        None
    }
}

/// The settings that join [`SSID`]: its name, an empty password, then `SAVE`.
pub fn join_lines() -> Vec<u8> {
    format!("SET\twifi_ssid\t{SSID}\nSET\twifi_pass\t\nSAVE\n").into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_once_when_wifi_is_unset() {
        let mut j = Joiner::new();
        assert_eq!(j.feed(b"rst:0x1\r\nSETTINGS\twifi_ssid=un"), None);
        let sent = j.feed(b"set\twifi_pass=unset\r\nSETUP\twaiting\n").unwrap();
        assert_eq!(
            sent,
            b"SET\twifi_ssid\tPICSimLabWifi\nSET\twifi_pass\t\nSAVE\n".to_vec()
        );
        assert!(j.done());
        assert_eq!(j.feed(b"SETTINGS\twifi_ssid=unset\n"), None);
    }

    #[test]
    fn leaves_set_wifi_and_other_sketches_alone() {
        let mut j = Joiner::new();
        assert_eq!(j.feed(b"SETTINGS\twifi_ssid=set\twifi_pass=set\n"), None);
        assert_eq!(j.feed(b"SETTINGS\tapi_key=unset\n"), None);
        assert_eq!(j.feed(b"hello wifi_ssid=unset\n"), None);
        assert!(!j.done());
    }

    #[test]
    fn network_args_are_restricted_and_forward_only_port_80() {
        let n = Network { web_port: 18080 };
        let a: Vec<String> = n
            .qemu_args()
            .into_iter()
            .map(|s| s.into_string().unwrap())
            .collect();
        assert_eq!(
            a[1],
            "user,id=esp32bench-net,restrict=on,ipv6=off,hostfwd=tcp:127.0.0.1:18080-:80"
        );
        assert_eq!(
            a[3],
            "hubport,id=esp32bench-hub,hubid=0,netdev=esp32bench-net"
        );
        assert_eq!(n.web_url(), "http://127.0.0.1:18080/");
    }
}
