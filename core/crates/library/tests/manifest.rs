//! The project manifest format: the schema, its example, and this crate's reader.
//! Set ESP32BENCH_TEST_MANIFEST to a real site build's desktop-manifest.json to check
//! that too.

use std::path::PathBuf;

use esp32bench_library::{Cache, Error, Firmware, parse};
use serde_json::Value;

fn schema_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../schema/project-manifest")
}

fn read(path: PathBuf) -> Vec<u8> {
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn assert_matches_schema(bytes: &[u8], what: &str) {
    let schema: Value = serde_json::from_slice(&read(schema_dir().join("v1.schema.json"))).unwrap();
    let instance: Value = serde_json::from_slice(bytes).unwrap();
    let v = jsonschema::validator_for(&schema).unwrap();
    let errors: Vec<String> = v
        .iter_errors(&instance)
        .take(10)
        .map(|e| format!("{e} at {}", e.instance_path()))
        .collect();
    assert!(errors.is_empty(), "{what}:\n{}", errors.join("\n"));
}

#[test]
fn example_matches_schema_and_parses() {
    let bytes = read(schema_dir().join("example.json"));
    assert_matches_schema(&bytes, "example.json");
    let m = parse(&bytes).unwrap();
    assert_eq!(m.projects.len(), 2);
    let s = m.summaries();
    let sketch = s.iter().find(|p| p.firmware == Firmware::Sketch).unwrap();
    assert!(!sketch.boards.is_empty());
    // Chips are derived from the boards list, without repeats.
    let mut seen = sketch.chips.clone();
    seen.dedup();
    assert_eq!(seen.len(), sketch.chips.len());
    assert!(sketch.chips.iter().all(|c| c.starts_with("esp32")));
    // A sketch for the ESP32 without Bluetooth runs in the simulator; prebuilt
    // firmware the library does not link does not.
    assert!(sketch.simulator);
    let prebuilt = s.iter().find(|p| p.firmware == Firmware::Prebuilt).unwrap();
    assert!(!prebuilt.simulator);
}

#[test]
fn bluetooth_projects_are_not_simulated() {
    let bytes = read(schema_dir().join("example.json"));
    let mut m = parse(&bytes).unwrap();
    let blink = m.projects.iter_mut().find(|p| p.id == "blink").unwrap();
    assert!(!esp32bench_library::uses_bluetooth(blink));
    blink.builds[0].code = Some("#include <BLEDevice.h>\nvoid setup() {}\nvoid loop() {}\n".into());
    assert!(esp32bench_library::uses_bluetooth(blink));
    let s = m.summaries();
    assert!(!s.iter().find(|p| p.id == "blink").unwrap().simulator);
}

#[test]
fn real_site_manifest_matches_schema() {
    let Some(path) = std::env::var_os("ESP32BENCH_TEST_MANIFEST") else {
        eprintln!("skipping: ESP32BENCH_TEST_MANIFEST not set");
        return;
    };
    let bytes = read(path.into());
    assert_matches_schema(&bytes, "site manifest");
    let m = parse(&bytes).unwrap();
    assert!(!m.projects.is_empty());
    for p in &m.projects {
        for b in &p.builds {
            assert!(
                m.boards.iter().any(|x| x.id == b.board),
                "{}: unknown board {}",
                p.id,
                b.board
            );
        }
    }
}

#[test]
fn rejects_other_formats_and_major_versions() {
    assert!(matches!(
        parse(b"{\"format\":\"x\",\"version\":\"1.0.0\"}"),
        Err(Error::Format(_))
    ));
    assert!(matches!(parse(b"not json"), Err(Error::Format(_))));
    let mut v: Value = serde_json::from_slice(&read(schema_dir().join("example.json"))).unwrap();
    v["version"] = "2.0.0".into();
    match parse(&serde_json::to_vec(&v).unwrap()) {
        Err(Error::Version { found }) => assert_eq!(found, "2.0.0"),
        other => panic!("expected a version error, got {other:?}"),
    }
}

#[test]
fn reads_newer_minor_versions_with_unknown_fields() {
    let mut v: Value = serde_json::from_slice(&read(schema_dir().join("example.json"))).unwrap();
    v["version"] = "1.7.0".into();
    v["somethingNew"] = "ignored".into();
    v["projects"][0]["anotherNewField"] = 3.into();
    let m = parse(&serde_json::to_vec(&v).unwrap()).unwrap();
    assert_eq!(m.version, "1.7.0");
}

#[tokio::test]
async fn cache_stores_good_manifests_and_keeps_them_over_bad_ones() {
    let dir = tempfile::tempdir().unwrap();
    let cache = Cache::new(dir.path().join("c"));
    assert!(cache.load().await.unwrap().is_none());

    let good = read(schema_dir().join("example.json"));
    let src = dir.path().join("site.json");
    std::fs::write(&src, &good).unwrap();
    let m = cache.refresh(src.to_str().unwrap()).await.unwrap();
    assert_eq!(cache.load().await.unwrap().unwrap(), m);

    std::fs::write(&src, b"{\"format\":\"nope\"}").unwrap();
    assert!(cache.refresh(src.to_str().unwrap()).await.is_err());
    assert_eq!(
        cache.load().await.unwrap().unwrap(),
        m,
        "a bad download replaced the cache"
    );
}

#[tokio::test]
async fn refuses_non_https_urls() {
    for bad in [
        "http://esp32bench.dev/desktop-manifest.json",
        "file:///etc/passwd",
        "ftp://x/y",
    ] {
        assert!(
            matches!(
                esp32bench_library::fetch(bad).await,
                Err(Error::Download { .. })
            ),
            "{bad} was accepted"
        );
    }
}

#[test]
fn install_entries_give_arduino_library_names() {
    let m = parse(&read(schema_dir().join("example.json"))).unwrap();
    let i = &m.projects[0].install[0];
    assert_eq!(i.registry, "adafruit/Adafruit NeoPixel");
    assert_eq!(i.arduino_name(), "Adafruit NeoPixel");
}
