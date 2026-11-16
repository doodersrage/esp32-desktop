//! Toolchain status and installs against stand-ins: a scripted arduino-cli and
//! small package index files. QEMU state depends on the machine, so only its shape
//! is checked here.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use esp32bench_toolchain::{
    ARDUINO_ESP32_VERSION, Env, Id, InstallError, State, core_download_bytes, install,
    installed_core_version, status,
};
use serde_json::json;

#[test]
fn core_pin_matches_the_test_firmware_pin() {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../tests/firmware/ARDUINO_ESP32_VERSION");
    assert_eq!(
        std::fs::read_to_string(p).unwrap().trim(),
        ARDUINO_ESP32_VERSION
    );
}

#[test]
fn reads_the_installed_core_version() {
    let list = json!({ "platforms": [
        { "id": "arduino:avr", "installed_version": "1.8.7" },
        { "id": "esp32:esp32", "installed_version": "3.3.12" }
    ]});
    assert_eq!(installed_core_version(&list).as_deref(), Some("3.3.12"));
    assert_eq!(installed_core_version(&json!({ "platforms": [] })), None);
    assert_eq!(installed_core_version(&json!({})), None);
}

/// Package index files: the platform (100 bytes), one esp32 tool with a Linux build
/// (50), one tool from the arduino index (7), one with no Linux build (ignored), and
/// one whose size is a JSON number (20).
fn write_index(dir: &Path) {
    let esp32 = json!({ "packages": [{
        "name": "esp32",
        "platforms": [
            { "version": "3.0.0", "size": "999", "toolsDependencies": [] },
            { "version": ARDUINO_ESP32_VERSION, "size": "100", "toolsDependencies": [
                { "packager": "esp32", "name": "gcc", "version": "1" },
                { "packager": "arduino", "name": "dfu-util", "version": "2" },
                { "packager": "esp32", "name": "mac-only", "version": "3" },
                { "packager": "esp32", "name": "libs", "version": "4" }
            ]}
        ],
        "tools": [
            { "name": "gcc", "version": "1", "systems": [
                { "host": "i686-mingw32", "size": "1000" },
                { "host": "x86_64-pc-linux-gnu", "size": "50" } ] },
            { "name": "mac-only", "version": "3", "systems": [ { "host": "arm64-apple-darwin", "size": "5" } ] },
            // Some tools give the size as a number, not a string.
            { "name": "libs", "version": "4", "systems": [ { "host": "x86_64-pc-linux-gnu", "size": 20 } ] }
        ]
    }]});
    let arduino = json!({ "packages": [{
        "name": "arduino", "platforms": [],
        "tools": [ { "name": "dfu-util", "version": "2", "systems": [ { "host": "x86_64-linux-gnu", "size": "7" } ] } ]
    }]});
    std::fs::write(dir.join("package_esp32_index.json"), esp32.to_string()).unwrap();
    std::fs::write(dir.join("package_index.json"), arduino.to_string()).unwrap();
}

#[test]
fn sums_the_core_download_for_this_host() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(
        core_download_bytes(dir.path(), ARDUINO_ESP32_VERSION),
        None,
        "no index yet"
    );
    write_index(dir.path());
    assert_eq!(
        core_download_bytes(dir.path(), ARDUINO_ESP32_VERSION),
        Some(177)
    );
    assert_eq!(core_download_bytes(dir.path(), "9.9.9"), None);
}

/// A stand-in arduino-cli reporting `core_version` (or no esp32 core), logging its
/// arguments to `log`.
fn fake_cli(dir: &Path, core_version: Option<&str>, log: &Path) -> PathBuf {
    let platforms = match core_version {
        Some(v) => format!(r#"{{"id":"esp32:esp32","installed_version":"{v}"}}"#),
        None => String::new(),
    };
    let script = format!(
        r#"#!/bin/sh
echo "$*" >> '{log}'
case "$1 $2" in
  "version --format") echo '{{"VersionString":"1.5.1"}}' ;;
  "core list") echo '{{"platforms":[{platforms}]}}' ;;
  "core update-index") echo "Downloading index" ;;
  "core install") echo "Installing $3" ;;
esac
"#,
        log = log.display()
    );
    let path = dir.join("arduino-cli");
    std::fs::write(&path, script).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn env(dir: &Path, cli: Option<PathBuf>) -> Env {
    Env {
        arduino_cli: cli,
        arduino_data: dir.to_owned(),
        sketchbook: dir.join("sketchbook"),
    }
}

#[tokio::test]
async fn reports_installed_core() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("log");
    let cli = fake_cli(dir.path(), Some(ARDUINO_ESP32_VERSION), &log);
    let s = status(&env(dir.path(), Some(cli))).await;
    let ids: Vec<Id> = s.iter().map(|c| c.id).collect();
    assert_eq!(ids, Id::ALL);
    assert_eq!(s[4].needed_for, "hardware");

    assert_eq!(s[0].state, State::Installed);
    assert_eq!(s[0].version.as_deref(), Some("1.5.1"));
    assert_eq!(s[1].state, State::Installed);
    assert_eq!(s[1].download_bytes, None);
    // QEMU depends on this machine; check only that it is described honestly.
    for q in &s[2..4] {
        assert_eq!(q.needed_for, "simulator");
        assert!(q.installable);
        assert_eq!(q.state == State::Installed, q.download_bytes.is_none());
    }
}

#[tokio::test]
async fn reports_wrong_and_missing_core_with_its_download_size() {
    let dir = tempfile::tempdir().unwrap();
    write_index(dir.path());
    let log = dir.path().join("log");

    let cli = fake_cli(dir.path(), Some("3.0.0"), &log);
    let s = status(&env(dir.path(), Some(cli))).await;
    assert_eq!(s[1].state, State::OtherVersion);
    assert_eq!(s[1].version.as_deref(), Some("3.0.0"));
    assert_eq!(s[1].download_bytes, Some(177));
    assert!(s[1].installable);

    let cli = fake_cli(dir.path(), None, &log);
    let s = status(&env(dir.path(), Some(cli))).await;
    assert_eq!(s[1].state, State::Missing);
}

#[tokio::test]
async fn without_arduino_cli_says_how_to_get_it_and_never_runs_it() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("no-such-arduino-cli");
    let s = status(&env(dir.path(), Some(missing.clone()))).await;
    assert_eq!(s[0].state, State::Missing);
    assert!(!s[0].installable);
    assert!(
        s[0].hint
            .as_deref()
            .unwrap()
            .contains("pacman -S arduino-cli")
    );
    assert_eq!(s[1].state, State::Missing);
    assert!(
        !s[1].installable,
        "the core cannot be installed without arduino-cli"
    );

    let r = install(&env(dir.path(), Some(missing)), Id::ArduinoCli, |_| {}).await;
    assert!(matches!(
        r,
        Err(InstallError::NotInstallable(Id::ArduinoCli))
    ));
}

#[tokio::test]
async fn installs_the_pinned_core_with_arduino_cli() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("log");
    let cli = fake_cli(dir.path(), None, &log);
    let mut lines = Vec::new();
    install(&env(dir.path(), Some(cli)), Id::Esp32Core, |l| {
        lines.push(l)
    })
    .await
    .unwrap();
    let calls = std::fs::read_to_string(&log).unwrap();
    let url = esp32bench_toolchain::ARDUINO_ESP32_INDEX_URL;
    assert_eq!(
        calls.lines().collect::<Vec<_>>(),
        [
            format!("core update-index --additional-urls {url}"),
            format!("core install esp32:esp32@{ARDUINO_ESP32_VERSION} --additional-urls {url}"),
        ]
    );
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("Installing esp32:esp32@"))
    );
}

#[test]
fn ids_round_trip() {
    for id in Id::ALL {
        assert_eq!(Id::parse(id.as_str()), Some(id));
    }
    assert_eq!(Id::parse("gcc"), None);
}

#[tokio::test]
async fn a_failing_arduino_cli_is_broken_not_missing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("arduino-cli");
    std::fs::write(
        &path,
        "#!/bin/sh\necho '{\"error\": \"Error: invalid path creating config dir\"}' >&2\nexit 10\n",
    )
    .unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    let s = status(&env(dir.path(), Some(path.clone()))).await;
    assert_eq!(s[0].state, State::Broken);
    assert_eq!(s[0].path.as_deref(), Some(path.as_path()));
    let hint = s[0].hint.as_deref().unwrap();
    assert!(hint.contains("invalid path creating config dir"), "{hint}");
    assert!(
        !hint.contains('{'),
        "the JSON wrapper leaked into the hint: {hint}"
    );
    assert!(
        !hint.contains("pacman"),
        "told to install something already installed"
    );
    assert!(!s[1].installable);
}

#[test]
fn board_access_needs_the_rule_or_the_group() {
    use esp32bench_toolchain::board_access;
    assert_eq!(board_access(true, false).state, State::Installed);
    assert_eq!(board_access(false, true).state, State::Installed);
    let none = board_access(false, false);
    assert_eq!(none.state, State::Missing);
    assert!(!none.installable, "needs root, so the app never does it");
    assert!(none.hint.unwrap().contains("usermod -aG uucp"));
}

#[test]
fn group_membership_matches_id() {
    let out = std::process::Command::new("id")
        .arg("-nG")
        .output()
        .unwrap();
    let groups = String::from_utf8_lossy(&out.stdout).to_string();
    let first = groups.split_whitespace().next().unwrap();
    assert!(
        esp32bench_toolchain::in_group(first),
        "not in my own group {first}"
    );
    assert!(!esp32bench_toolchain::in_group("no-such-group-esp32bench"));
}
