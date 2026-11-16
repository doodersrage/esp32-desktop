//! packaging/udev/69-esp32bench.rules against the USB devices the core recognises,
//! and the packaging rules: uaccess and the uucp group, never world-writable.

use std::path::PathBuf;

use esp32bench_device::KNOWN_USB;

fn rules() -> (PathBuf, String) {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../packaging/udev/69-esp32bench.rules");
    let text = std::fs::read_to_string(&p).unwrap();
    (p, text)
}

fn attr(line: &str, key: &str) -> Option<String> {
    let start = line.find(&format!("{key}==\""))? + key.len() + 3;
    Some(line[start..].split('"').next()?.to_owned())
}

#[test]
fn every_known_device_has_a_tty_rule() {
    let (_, text) = rules();
    let tty: Vec<&str> = text
        .lines()
        .filter(|l| l.starts_with("SUBSYSTEM==\"tty\""))
        .collect();
    for (vid, pid, bridge, _) in KNOWN_USB {
        let vid = format!("{vid:04x}");
        let covered = tty.iter().any(|l| {
            attr(l, "ATTRS{idVendor}").as_deref() == Some(&vid)
                && match (pid, attr(l, "ATTRS{idProduct}")) {
                    (_, None) => true, // a vendor-wide rule covers every product
                    (Some(p), Some(rp)) => rp == format!("{p:04x}"),
                    (None, Some(_)) => false,
                }
        });
        assert!(covered, "no udev rule for {vid}:{pid:?} ({bridge:?})");
    }
}

#[test]
fn rules_grant_seat_and_group_access_only() {
    let (_, text) = rules();
    let rules: Vec<&str> = text
        .lines()
        .filter(|l| l.starts_with("SUBSYSTEM=="))
        .collect();
    assert!(!rules.is_empty());
    for l in rules {
        assert!(l.contains("TAG+=\"uaccess\""), "missing uaccess: {l}");
        assert!(l.contains("GROUP=\"uucp\""), "missing uucp group: {l}");
        assert!(l.contains("MODE=\"0660\""), "mode must be 0660: {l}");
        assert!(
            !l.contains("0666") && !l.contains("0777"),
            "world-writable: {l}"
        );
    }
}

#[test]
fn udevadm_accepts_the_rules() {
    let (path, _) = rules();
    let Ok(out) = std::process::Command::new("udevadm")
        .arg("verify")
        .arg(&path)
        .output()
    else {
        eprintln!("skipping: no udevadm");
        return;
    };
    if String::from_utf8_lossy(&out.stderr).contains("Unknown command") {
        eprintln!("skipping: this udevadm has no verify");
        return;
    }
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
