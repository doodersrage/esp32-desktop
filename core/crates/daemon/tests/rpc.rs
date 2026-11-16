//! The daemon against its contract in `schema/rpc/`, over a real Unix socket.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use esp32bench_rpc::{Client, ClientError, code};
use serde_json::{Value, json};

fn schema_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../schema/rpc")
}

fn read_json(path: &Path) -> Value {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn index() -> Value {
    read_json(&schema_dir().join("index.json"))
}

fn method_def(method: &str) -> Value {
    read_json(&schema_dir().join(format!("{method}.json")))
}

fn assert_valid(schema: &Value, instance: &Value, what: &str) {
    let validator =
        jsonschema::validator_for(schema).unwrap_or_else(|e| panic!("{what}: bad schema: {e}"));
    let errors: Vec<String> = validator
        .iter_errors(instance)
        .map(|e| format!("{} at {}", e, e.instance_path()))
        .collect();
    assert!(
        errors.is_empty(),
        "{what}: {instance}\n{}",
        errors.join("\n")
    );
}

/// The daemon's router on a temporary socket, through the same `bind` as the binary.
async fn daemon() -> (tempfile::TempDir, PathBuf, esp32bench_daemon::InstanceLock) {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("rt/core.sock");
    let (listener, lock) = esp32bench_daemon::bind(&sock).unwrap();
    tokio::spawn(esp32bench_rpc::serve(
        listener,
        Arc::new(esp32bench_daemon::router()),
    ));
    (dir, sock, lock)
}

#[test]
fn protocol_version_matches_index() {
    assert_eq!(index()["protocol"], esp32bench_daemon::PROTOCOL);
}

#[test]
fn served_methods_match_the_schema_index() {
    let mut listed: Vec<String> = index()["methods"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m.as_str().unwrap().to_owned())
        .collect();
    listed.sort();
    assert_eq!(esp32bench_daemon::router().names(), listed);
    for m in &listed {
        assert_eq!(
            method_def(m)["method"],
            m.as_str(),
            "{m}.json names another method"
        );
    }
}

#[test]
fn every_example_matches_its_schema() {
    for m in index()["methods"].as_array().unwrap() {
        let m = m.as_str().unwrap();
        let def = method_def(m);
        let examples = def["examples"]
            .as_array()
            .unwrap_or_else(|| panic!("{m}: no examples"));
        assert!(!examples.is_empty(), "{m}: no examples");
        for (i, ex) in examples.iter().enumerate() {
            assert_valid(
                &def["params"],
                &ex["params"],
                &format!("{m} example {i} params"),
            );
            assert_valid(
                &def["result"],
                &ex["result"],
                &format!("{m} example {i} result"),
            );
        }
    }
}

#[tokio::test]
async fn core_hello_round_trip() {
    let (_d, sock, _lock) = daemon().await;
    let mut c = Client::connect(&sock).await.unwrap();
    let def = method_def("core.hello");
    let params = json!({ "client": "test", "protocol": "0.1.0" });
    assert_valid(&def["params"], &params, "core.hello params");
    let r = c.call("core.hello", params).await.unwrap();
    assert_valid(&def["result"], &r, "core.hello result");
    assert_eq!(r["protocol"], esp32bench_daemon::PROTOCOL);

    match c.call("core.hello", json!({ "bogus": 1 })).await {
        Err(ClientError::Rpc(e)) => assert_eq!(e.code, code::INVALID_PARAMS),
        other => panic!("expected invalid params, got {other:?}"),
    }
}

#[tokio::test]
async fn device_list_round_trip() {
    let (_d, sock, _lock) = daemon().await;
    let mut c = Client::connect(&sock).await.unwrap();
    let def = method_def("device.list");
    // Whatever ports this machine has, the result must match the schema.
    for params in [json!({}), json!({ "all": true })] {
        let r = c.call("device.list", params.clone()).await.unwrap();
        assert_valid(&def["result"], &r, &format!("device.list {params} result"));
    }
    match c.call("device.list", json!({ "all": "yes" })).await {
        Err(ClientError::Rpc(e)) => assert_eq!(e.code, code::INVALID_PARAMS),
        other => panic!("expected invalid params, got {other:?}"),
    }
}

#[tokio::test]
async fn second_daemon_cannot_take_the_socket() {
    let (_d, sock, lock) = daemon().await;
    match esp32bench_daemon::bind(&sock) {
        Err(e) => assert_eq!(e.kind(), std::io::ErrorKind::AddrInUse),
        Ok(_) => panic!("a second daemon bound the socket"),
    }
    // The first daemon still answers.
    let mut c = Client::connect(&sock).await.unwrap();
    c.call("core.hello", json!({})).await.unwrap();
    // Releasing the lock removes the socket, and a new daemon can start.
    drop(c);
    drop(lock);
    assert!(!sock.exists());
    let (_l, _lock2) = esp32bench_daemon::bind(&sock).unwrap();
}

#[test]
fn socket_is_private() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("rt/core.sock");
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let (_l, _lock) = esp32bench_daemon::bind(&sock).unwrap();
    let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&sock), 0o600);
    assert_eq!(mode(sock.parent().unwrap()), 0o700);
}
