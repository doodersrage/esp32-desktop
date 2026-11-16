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

/// State with a private library cache, fetching from `source`.
fn state(dir: &Path, source: &Path) -> Arc<esp32bench_daemon::State> {
    Arc::new(esp32bench_daemon::State::new(
        esp32bench_library::Cache::new(dir.join("cache")),
        source.to_string_lossy().into_owned(),
    ))
}

fn example_manifest() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../schema/project-manifest/example.json")
}

/// The daemon's router on a temporary socket, through the same `bind` as the binary.
/// Its library source is the schema's example manifest.
async fn daemon() -> (tempfile::TempDir, PathBuf, esp32bench_daemon::InstanceLock) {
    daemon_with_source(&example_manifest()).await
}

async fn daemon_with_source(
    source: &Path,
) -> (tempfile::TempDir, PathBuf, esp32bench_daemon::InstanceLock) {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("rt/core.sock");
    let (listener, lock) = esp32bench_daemon::bind(&sock).unwrap();
    let router = esp32bench_daemon::router(state(dir.path(), source));
    tokio::spawn(esp32bench_rpc::serve(listener, Arc::new(router)));
    (dir, sock, lock)
}

fn router_names() -> Vec<String> {
    let dir = tempfile::tempdir().unwrap();
    esp32bench_daemon::router(state(dir.path(), &example_manifest())).names()
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
    assert_eq!(router_names(), listed);
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

#[test]
fn every_event_example_matches_its_schema() {
    let events = index()["events"].as_array().cloned().unwrap_or_default();
    assert!(!events.is_empty());
    for e in &events {
        let e = e.as_str().unwrap();
        let def = method_def(e);
        assert_eq!(def["event"], e, "{e}.json names another event");
        for (i, ex) in def["examples"].as_array().unwrap().iter().enumerate() {
            assert_valid(&def["params"], &ex["params"], &format!("{e} example {i}"));
        }
    }
    // Every event a method announces is defined.
    for m in index()["methods"].as_array().unwrap() {
        for e in method_def(m.as_str().unwrap())["events"]
            .as_array()
            .cloned()
            .unwrap_or_default()
        {
            assert!(events.contains(&e), "{m} announces undefined event {e}");
        }
    }
}

#[test]
fn port_schemas_are_identical_everywhere() {
    let list = &method_def("device.list")["result"]["properties"]["ports"];
    assert_eq!(
        &method_def("device.watch")["result"]["properties"]["ports"],
        list
    );
    assert_eq!(
        &method_def("device.changed")["params"]["properties"]["ports"],
        list
    );
}

#[tokio::test]
async fn device_watch_and_unsubscribe_round_trip() {
    let (_d, sock, _lock) = daemon().await;
    let mut c = Client::connect(&sock).await.unwrap();
    let r = c.call("device.watch", json!({})).await.unwrap();
    assert_valid(
        &method_def("device.watch")["result"],
        &r,
        "device.watch result",
    );
    let id = r["subscription"].as_u64().unwrap();

    let r = c
        .call("core.unsubscribe", json!({ "subscription": id }))
        .await
        .unwrap();
    assert_valid(
        &method_def("core.unsubscribe")["result"],
        &r,
        "core.unsubscribe result",
    );
    // A second unsubscribe, or an id from nowhere, is "not found".
    for bad in [id, 999] {
        match c
            .call("core.unsubscribe", json!({ "subscription": bad }))
            .await
        {
            Err(ClientError::Rpc(e)) => assert_eq!(e.code, code::NOT_FOUND),
            other => panic!("expected not found, got {other:?}"),
        }
    }
}

#[tokio::test]
async fn core_hello_round_trip() {
    let (_d, sock, _lock) = daemon().await;
    let mut c = Client::connect(&sock).await.unwrap();
    let def = method_def("core.hello");
    let params = json!({ "client": "test", "protocol": "0.4.0" });
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

#[tokio::test]
async fn library_refresh_then_list() {
    let (_d, sock, _lock) = daemon().await;
    let mut c = Client::connect(&sock).await.unwrap();

    let empty = c.call("library.list", json!({})).await.unwrap();
    assert_valid(
        &method_def("library.list")["result"],
        &empty,
        "library.list before refresh",
    );
    assert_eq!(empty["available"], false);

    let r = c.call("library.refresh", json!({})).await.unwrap();
    assert_valid(
        &method_def("library.refresh")["result"],
        &r,
        "library.refresh result",
    );
    let job = r["job"].clone();
    let n = tokio::time::timeout(std::time::Duration::from_secs(10), c.next_notification())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(n["method"], "library.refreshed");
    assert_valid(
        &method_def("library.refreshed")["params"],
        &n["params"],
        "library.refreshed",
    );
    assert_eq!(n["params"]["subscription"], job);
    assert_eq!(n["params"]["ok"], true);
    assert_eq!(n["params"]["projects"], 2);

    let full = c.call("library.list", json!({})).await.unwrap();
    assert_valid(
        &method_def("library.list")["result"],
        &full,
        "library.list after refresh",
    );
    assert_eq!(full["available"], true);
    assert_eq!(full["projects"].as_array().unwrap().len(), 2);
    assert!(full["fetchedAt"].as_u64().unwrap() > 0);
}

#[tokio::test]
async fn failed_refresh_reports_and_keeps_the_cache() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("site.json");
    std::fs::copy(example_manifest(), &src).unwrap();
    let (_d, sock, _lock) = daemon_with_source(&src).await;
    let mut c = Client::connect(&sock).await.unwrap();

    c.call("library.refresh", json!({})).await.unwrap();
    let ok = c.next_notification().await.unwrap();
    assert_eq!(ok["params"]["ok"], true);

    std::fs::write(&src, b"{\"format\": \"something else\"}").unwrap();
    c.call("library.refresh", json!({})).await.unwrap();
    let bad = c.next_notification().await.unwrap();
    assert_valid(
        &method_def("library.refreshed")["params"],
        &bad["params"],
        "failed refresh",
    );
    assert_eq!(bad["params"]["ok"], false);
    assert!(
        bad["params"]["error"]
            .as_str()
            .unwrap()
            .contains("not a project manifest")
    );

    let list = c.call("library.list", json!({})).await.unwrap();
    assert_eq!(list["available"], true, "the good cache was lost");
}

#[tokio::test]
async fn job_cancel_round_trip() {
    let (_d, sock, _lock) = daemon().await;
    let mut c = Client::connect(&sock).await.unwrap();
    match c.call("job.cancel", json!({ "job": 12345 })).await {
        Err(ClientError::Rpc(e)) => assert_eq!(e.code, code::NOT_FOUND),
        other => panic!("expected not found, got {other:?}"),
    }
    let job = c.call("library.refresh", json!({})).await.unwrap()["job"].clone();
    let r = c.call("job.cancel", json!({ "job": job })).await.unwrap();
    assert_valid(&method_def("job.cancel")["result"], &r, "job.cancel result");
}

#[test]
fn library_get_uses_the_manifest_project_schema() {
    let manifest: Value = serde_json::from_slice(
        &std::fs::read(example_manifest().with_file_name("v1.schema.json")).unwrap(),
    )
    .unwrap();
    let get = method_def("library.get");
    assert_eq!(
        get["result"]["properties"]["project"],
        manifest["properties"]["projects"]["items"]
    );
    assert_eq!(
        get["result"]["properties"]["boards"]["items"],
        manifest["properties"]["boards"]["items"]
    );
}

#[tokio::test]
async fn library_get_round_trip() {
    let (_d, sock, _lock) = daemon().await;
    let mut c = Client::connect(&sock).await.unwrap();
    // Nothing cached yet.
    match c.call("library.get", json!({ "id": "blink" })).await {
        Err(ClientError::Rpc(e)) => assert_eq!(e.code, code::NOT_FOUND),
        other => panic!("expected not found, got {other:?}"),
    }
    c.call("library.refresh", json!({})).await.unwrap();
    c.next_notification().await.unwrap();

    let r = c
        .call("library.get", json!({ "id": "blink" }))
        .await
        .unwrap();
    assert_valid(
        &method_def("library.get")["result"],
        &r,
        "library.get result",
    );
    assert_eq!(r["project"]["id"], "blink");
    // Exactly the boards the project's builds use.
    let mut used: Vec<&str> = r["project"]["builds"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["board"].as_str().unwrap())
        .collect();
    let mut got: Vec<&str> = r["boards"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["id"].as_str().unwrap())
        .collect();
    used.sort();
    got.sort();
    assert_eq!(used, got);

    match c
        .call("library.get", json!({ "id": "no-such-project" }))
        .await
    {
        Err(ClientError::Rpc(e)) => assert_eq!(e.code, code::NOT_FOUND),
        other => panic!("expected not found, got {other:?}"),
    }
}
