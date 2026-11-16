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
    let mut s = esp32bench_daemon::State::new(
        esp32bench_library::Cache::new(dir.join("cache")),
        source.to_string_lossy().into_owned(),
        dir.join("builds"),
        dir.join("sketchbook"),
    );
    s.arduino_cli = Some(fake_arduino_cli(dir));
    Arc::new(s)
}

/// A stand-in arduino-cli: installs nothing, and "compiles" by printing a line and
/// writing empty output files where arduino-cli would.
fn fake_arduino_cli(dir: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join("fake-arduino-cli");
    std::fs::write(
        &path,
        r#"#!/bin/sh
case "$1" in
  lib) echo "Installed $3" ;;
  compile)
    out=""; prev=""
    for a in "$@"; do [ "$prev" = "--output-dir" ] && out="$a"; prev="$a"; last="$a"; done
    name=$(basename "$last")
    echo "Sketch uses 1234 bytes"
    : > "$out/$name.ino.merged.bin"; : > "$out/$name.ino.elf" ;;
esac
"#,
    )
    .unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
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
    let params = json!({ "client": "test", "protocol": "0.8.0" });
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

#[test]
fn every_error_example_matches_its_error_data_schema() {
    for m in index()["methods"].as_array().unwrap() {
        let m = m.as_str().unwrap();
        let def = method_def(m);
        for (i, ex) in def["errorExamples"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .enumerate()
        {
            let code = ex["code"].as_i64().unwrap();
            assert!(
                def["errors"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|c| c.as_i64() == Some(code)),
                "{m} error example {i}: code {code} is not in errors"
            );
            let schema = &def["errorData"][code.to_string()];
            assert!(!schema.is_null(), "{m}: no errorData schema for {code}");
            assert_valid(schema, &ex["data"], &format!("{m} error example {i}"));
        }
    }
}

/// Download the example library into the daemon, so projects can be built.
async fn with_library(c: &mut Client) {
    c.call("library.refresh", json!({})).await.unwrap();
    let n = c.next_notification().await.unwrap();
    assert_eq!(n["params"]["ok"], true);
}

#[tokio::test]
async fn build_needs_consent_for_the_exact_plan() {
    let (_d, sock, _lock) = daemon().await;
    let mut c = Client::connect(&sock).await.unwrap();
    with_library(&mut c).await;
    let def = method_def("build.start");

    // No token: refused, with a token and the plan to show the user.
    let refusal = match c
        .call(
            "build.start",
            json!({ "project": "blink", "board": "esp32" }),
        )
        .await
    {
        Err(ClientError::Rpc(e)) => e,
        other => panic!("expected consent required, got {other:?}"),
    };
    assert_eq!(refusal.code, code::CONSENT_REQUIRED);
    let data = refusal.data.unwrap();
    assert_valid(&def["errorData"]["-32002"], &data, "consent data");
    assert_eq!(data["plan"]["install"], json!(["Adafruit NeoPixel"]));
    let token = data["token"].as_str().unwrap().to_owned();

    // A token for one board does not build another.
    match c
        .call(
            "build.start",
            json!({ "project": "blink", "board": "esp32-doit", "consent": token }),
        )
        .await
    {
        Err(ClientError::Rpc(e)) => assert_eq!(e.code, code::CONSENT_REQUIRED),
        other => panic!("expected consent required, got {other:?}"),
    }
    // ...and that attempt used it up.
    match c
        .call(
            "build.start",
            json!({ "project": "blink", "board": "esp32", "consent": token }),
        )
        .await
    {
        Err(ClientError::Rpc(e)) => assert_eq!(e.code, code::CONSENT_REQUIRED),
        other => panic!("a used token started a build: {other:?}"),
    }
}

#[tokio::test]
async fn build_runs_after_consent_and_reports() {
    let (_d, sock, _lock) = daemon().await;
    let mut c = Client::connect(&sock).await.unwrap();
    with_library(&mut c).await;

    let token = match c
        .call(
            "build.start",
            json!({ "project": "blink", "board": "esp32" }),
        )
        .await
    {
        Err(ClientError::Rpc(e)) => e.data.unwrap()["token"].as_str().unwrap().to_owned(),
        other => panic!("expected consent required, got {other:?}"),
    };
    let r = c
        .call(
            "build.start",
            json!({ "project": "blink", "board": "esp32", "consent": token }),
        )
        .await
        .unwrap();
    assert_valid(
        &method_def("build.start")["result"],
        &r,
        "build.start result",
    );
    let job = r["job"].clone();

    let mut saw_log = false;
    loop {
        let n = tokio::time::timeout(std::time::Duration::from_secs(10), c.next_notification())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(n["params"]["subscription"], job);
        let event = n["method"].as_str().unwrap();
        assert_valid(&method_def(event)["params"], &n["params"], event);
        if event == "build.log" {
            saw_log = true;
            continue;
        }
        assert_eq!(event, "build.finished");
        assert_eq!(n["params"]["ok"], true, "{n}");
        assert!(Path::new(n["params"]["image"].as_str().unwrap()).is_file());
        break;
    }
    assert!(saw_log);
}

#[tokio::test]
async fn build_refuses_prebuilt_and_unknown_projects() {
    let (_d, sock, _lock) = daemon().await;
    let mut c = Client::connect(&sock).await.unwrap();
    with_library(&mut c).await;
    for (project, board, want) in [
        ("probeharbor-probe", "esp32", code::FAILED),
        ("no-such-project", "esp32", code::NOT_FOUND),
        ("blink", "esp32c6", code::NOT_FOUND),
        ("../../etc", "esp32", code::INVALID_PARAMS),
    ] {
        match c
            .call("build.start", json!({ "project": project, "board": board }))
            .await
        {
            Err(ClientError::Rpc(e)) => {
                assert_eq!(e.code, want, "{project}/{board}: {}", e.message)
            }
            other => panic!("{project}/{board}: expected an error, got {other:?}"),
        }
    }
}

/// A pty pair: the master fd (the "board") and the slave path (the "port").
fn pty() -> (std::os::fd::OwnedFd, PathBuf) {
    use std::os::fd::FromRawFd;
    let (mut master, mut slave) = (-1, -1);
    let mut name = [0 as libc::c_char; 128];
    // SAFETY: openpty with valid out-pointers and a large enough name buffer.
    let r = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            name.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    assert_eq!(r, 0);
    // SAFETY: fresh fds from openpty; the slave is reopened by path.
    unsafe { libc::close(slave) };
    let path = unsafe { std::ffi::CStr::from_ptr(name.as_ptr()) }
        .to_string_lossy()
        .into_owned();
    (
        unsafe { std::os::fd::OwnedFd::from_raw_fd(master) },
        PathBuf::from(path),
    )
}

/// A daemon whose serial.open may open `allowed` (a pty) in place of device.list ports.
async fn serial_daemon(
    allowed: &Path,
) -> (tempfile::TempDir, PathBuf, esp32bench_daemon::InstanceLock) {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("rt/core.sock");
    let (listener, lock) = esp32bench_daemon::bind(&sock).unwrap();
    let mut st = esp32bench_daemon::State::new(
        esp32bench_library::Cache::new(dir.path().join("cache")),
        example_manifest().to_string_lossy().into_owned(),
        dir.path().join("builds"),
        dir.path().join("sketchbook"),
    );
    st.serial_allow = Some(vec![allowed.to_owned()]);
    let router = esp32bench_daemon::router(Arc::new(st));
    tokio::spawn(esp32bench_rpc::serve(listener, Arc::new(router)));
    (dir, sock, lock)
}

fn b64(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn un_b64(s: &str) -> Vec<u8> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.decode(s).unwrap()
}

#[tokio::test]
async fn serial_streams_bytes_both_ways() {
    use std::os::fd::AsRawFd;
    let (master, port) = pty();
    let (_d, sock, _lock) = serial_daemon(&port).await;
    let mut c = Client::connect(&sock).await.unwrap();

    let r = c
        .call("serial.open", json!({ "path": port, "baud": 115200 }))
        .await
        .unwrap();
    assert_valid(
        &method_def("serial.open")["result"],
        &r,
        "serial.open result",
    );
    let sub = r["subscription"].as_u64().unwrap();

    // The board prints something that is not valid UTF-8.
    let out = b"boot ok \xff\n";
    // SAFETY: write a valid buffer to the pty master.
    unsafe { libc::write(master.as_raw_fd(), out.as_ptr().cast(), out.len()) };
    let mut got = Vec::new();
    while got.len() < out.len() {
        let n = tokio::time::timeout(std::time::Duration::from_secs(2), c.next_notification())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(n["method"], "serial.data");
        assert_valid(
            &method_def("serial.data")["params"],
            &n["params"],
            "serial.data",
        );
        assert_eq!(n["params"]["subscription"], sub);
        got.extend(un_b64(n["params"]["data"].as_str().unwrap()));
        assert!(n["params"]["text"].as_str().unwrap().starts_with("boot ok"));
    }
    assert_eq!(got, out, "raw bytes must arrive unchanged");

    let r = c
        .call(
            "serial.write",
            json!({ "subscription": sub, "data": b64(b"ping\n") }),
        )
        .await
        .unwrap();
    assert_valid(
        &method_def("serial.write")["result"],
        &r,
        "serial.write result",
    );
    let mut pfd = libc::pollfd {
        fd: master.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: poll and read on the valid pty master.
    assert!(unsafe { libc::poll(&mut pfd, 1, 1000) } > 0);
    let mut buf = [0u8; 16];
    let n = unsafe { libc::read(master.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
    assert_eq!(&buf[..n as usize], b"ping\n");

    // A pty has no modem lines, so reset fails cleanly rather than crashing.
    match c.call("serial.reset", json!({ "subscription": sub })).await {
        Ok(_) => {}
        Err(ClientError::Rpc(e)) => assert_eq!(e.code, code::FAILED),
        other => panic!("unexpected {other:?}"),
    }
}

#[tokio::test]
async fn serial_refuses_paths_and_other_connections() {
    let (_master, port) = pty();
    let (_d, sock, _lock) = serial_daemon(&port).await;
    let mut c = Client::connect(&sock).await.unwrap();

    // Only ports the core knows about; never an arbitrary file.
    for bad in ["/etc/passwd", "/dev/sda", "../../dev/ttyS0"] {
        match c
            .call("serial.open", json!({ "path": bad, "baud": 115200 }))
            .await
        {
            Err(ClientError::Rpc(e)) => assert_eq!(e.code, code::NOT_FOUND, "{bad}"),
            other => panic!("{bad} was opened: {other:?}"),
        }
    }
    match c
        .call("serial.open", json!({ "path": port, "baud": 1234 }))
        .await
    {
        Err(ClientError::Rpc(e)) => assert_eq!(e.code, code::INVALID_PARAMS),
        other => panic!("bad baud accepted: {other:?}"),
    }

    let sub = c
        .call("serial.open", json!({ "path": port, "baud": 115200 }))
        .await
        .unwrap()["subscription"]
        .as_u64()
        .unwrap();
    // The port is opened exclusively.
    match c
        .call("serial.open", json!({ "path": port, "baud": 115200 }))
        .await
    {
        Err(ClientError::Rpc(e)) => assert_eq!(e.code, code::BUSY, "{}", e.message),
        other => panic!("opened twice: {other:?}"),
    }
    // Another connection cannot write to this connection's port, even with its id.
    let mut other = Client::connect(&sock).await.unwrap();
    match other
        .call(
            "serial.write",
            json!({ "subscription": sub, "data": b64(b"x") }),
        )
        .await
    {
        Err(ClientError::Rpc(e)) => assert_eq!(e.code, code::NOT_FOUND),
        other => panic!("wrote to another connection's port: {other:?}"),
    }

    // Closing it frees the device for the next open.
    c.call("core.unsubscribe", json!({ "subscription": sub }))
        .await
        .unwrap();
    let mut reopened = false;
    for _ in 0..20 {
        if c.call("serial.open", json!({ "path": port, "baud": 115200 }))
            .await
            .is_ok()
        {
            reopened = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(reopened, "the port stayed busy after unsubscribe");
}

#[tokio::test]
async fn serial_reports_when_the_board_goes_away() {
    let (master, port) = pty();
    let (_d, sock, _lock) = serial_daemon(&port).await;
    let mut c = Client::connect(&sock).await.unwrap();
    c.call("serial.open", json!({ "path": port, "baud": 115200 }))
        .await
        .unwrap();
    drop(master); // unplugged
    loop {
        let n = tokio::time::timeout(std::time::Duration::from_secs(3), c.next_notification())
            .await
            .unwrap()
            .unwrap();
        if n["method"] == "serial.closed" {
            assert_valid(
                &method_def("serial.closed")["params"],
                &n["params"],
                "serial.closed",
            );
            break;
        }
    }
}

#[tokio::test]
async fn toolchain_status_matches_schema() {
    let (_d, sock, _lock) = daemon().await;
    let mut c = Client::connect(&sock).await.unwrap();
    let r = c.call("toolchain.status", json!({})).await.unwrap();
    assert_valid(
        &method_def("toolchain.status")["result"],
        &r,
        "toolchain.status result",
    );
    let ids: Vec<&str> = r["components"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        [
            "arduino-cli",
            "esp32-core",
            "qemu-xtensa",
            "qemu-riscv32",
            "device-access"
        ]
    );
}

#[tokio::test]
async fn toolchain_install_runs_as_a_job() {
    let (_d, sock, _lock) = daemon().await;
    let mut c = Client::connect(&sock).await.unwrap();
    // The daemon's stand-in arduino-cli accepts any command, so the core "installs".
    let r = c
        .call("toolchain.install", json!({ "component": "esp32-core" }))
        .await
        .unwrap();
    assert_valid(
        &method_def("toolchain.install")["result"],
        &r,
        "toolchain.install result",
    );
    loop {
        let n = tokio::time::timeout(std::time::Duration::from_secs(10), c.next_notification())
            .await
            .unwrap()
            .unwrap();
        let event = n["method"].as_str().unwrap();
        assert_valid(&method_def(event)["params"], &n["params"], event);
        if event == "toolchain.finished" {
            assert_eq!(n["params"]["ok"], true, "{n}");
            break;
        }
    }
    // arduino-cli itself needs root, so it is never installed from here.
    match c
        .call("toolchain.install", json!({ "component": "arduino-cli" }))
        .await
    {
        Err(ClientError::Rpc(e)) => {
            assert_eq!(e.code, code::FAILED);
            assert!(e.message.contains("pacman"));
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
    match c
        .call("toolchain.install", json!({ "component": "gcc" }))
        .await
    {
        Err(ClientError::Rpc(e)) => assert_eq!(e.code, code::INVALID_PARAMS),
        other => panic!("expected invalid params, got {other:?}"),
    }
}

/// QEMU in download mode with its UART on a pty, for board.* tests. Killed on drop.
struct DownloadModeQemu {
    child: std::process::Child,
    pty: PathBuf,
}

impl Drop for DownloadModeQemu {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Start `chip` (esp32 or esp32c3) in download mode over `flash`, or None without QEMU.
fn download_mode_qemu(chip: &str, flash: &Path, log: &Path) -> Option<DownloadModeQemu> {
    let (arch, strap) = match chip {
        "esp32" => (
            esp32bench_sim::Arch::Xtensa,
            "driver=esp32.gpio,property=strap_mode,value=0x0f",
        ),
        _ => (
            esp32bench_sim::Arch::Riscv32,
            "driver=esp32c3.gpio,property=strap_mode,value=0x02",
        ),
    };
    let qemu = esp32bench_sim::install::find_qemu(arch).ok()?;
    let out = std::fs::File::create(log).unwrap();
    let mut drive = std::ffi::OsString::from("file=");
    drive.push(flash);
    drive.push(",if=mtd,format=raw");
    let child = std::process::Command::new(qemu)
        .args(["-machine", chip, "-display", "none", "-monitor", "none"])
        .arg("-drive")
        .arg(drive)
        .args(["-global", strap, "-serial", "pty"])
        .stdout(out.try_clone().unwrap())
        .stderr(out)
        .spawn()
        .unwrap();
    let mut q = DownloadModeQemu {
        child,
        pty: PathBuf::new(),
    };
    for _ in 0..100 {
        let text = std::fs::read_to_string(log).unwrap_or_default();
        if let Some(i) = text.find("/dev/pts/") {
            q.pty = PathBuf::from(
                text[i..]
                    .chars()
                    .take_while(|c| !c.is_whitespace() && *c != ')')
                    .collect::<String>(),
            );
            return Some(q);
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    panic!("QEMU did not report its pty");
}

fn firmware(name: &str) -> Option<PathBuf> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../tests/firmware/out")
        .join(name);
    p.is_file().then_some(p)
}

/// Wait for a board job's events until board.finished; check each against its schema.
async fn board_events(c: &mut Client, job: &Value) -> (Vec<Value>, Value) {
    let mut progress = Vec::new();
    loop {
        let n = tokio::time::timeout(std::time::Duration::from_secs(120), c.next_notification())
            .await
            .expect("board job timed out")
            .unwrap();
        let event = n["method"].as_str().unwrap();
        assert_valid(&method_def(event)["params"], &n["params"], event);
        assert_eq!(&n["params"]["subscription"], job);
        if event == "board.finished" {
            return (progress, n["params"].clone());
        }
        progress.push(n["params"].clone());
    }
}

/// Params that reach QEMU: no reset lines on a pty, and the ROM loader (no stub).
fn qemu_params(pty: &Path, extra: Value) -> Value {
    let mut p = json!({ "port": pty, "reset": "none", "stub": false });
    p.as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    p
}

#[tokio::test]
async fn board_inspect_reads_chip_and_partitions_from_qemu() {
    let Some(hello) = firmware("hello-esp32.bin") else {
        eprintln!("skipping: hello firmware not built");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let flash = dir.path().join("flash.bin");
    std::fs::copy(hello, &flash).unwrap();
    let Some(q) = download_mode_qemu("esp32", &flash, &dir.path().join("q.log")) else {
        eprintln!("skipping: QEMU not installed");
        return;
    };
    let (_d, sock, _lock) = serial_daemon(&q.pty).await;
    let mut c = Client::connect(&sock).await.unwrap();
    let r = c
        .call("board.inspect", qemu_params(&q.pty, json!({})))
        .await
        .unwrap();
    assert_valid(
        &method_def("board.inspect")["result"],
        &r,
        "board.inspect result",
    );
    let (progress, done) = board_events(&mut c, &r["job"]).await;
    assert_eq!(done["ok"], true, "{done}");
    assert_eq!(done["info"]["chip"], "esp32");
    assert!(
        done["partitions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["subtype"] == "factory")
    );
    assert_eq!(progress[0]["stage"], "connecting");
}

#[tokio::test]
async fn board_flash_needs_consent_and_writes_the_build() {
    let (Some(hello), Some(gpio)) = (firmware("hello-esp32.bin"), firmware("gpio-esp32.bin"))
    else {
        eprintln!("skipping: test firmware not built");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let flash = dir.path().join("flash.bin");
    std::fs::copy(hello, &flash).unwrap();
    let Some(q) = download_mode_qemu("esp32", &flash, &dir.path().join("q.log")) else {
        eprintln!("skipping: QEMU not installed");
        return;
    };
    let (d, sock, _lock) = serial_daemon(&q.pty).await;
    let mut c = Client::connect(&sock).await.unwrap();
    with_library(&mut c).await;
    // Pretend the gpio firmware is the last build of blink for esp32.
    let out = d.path().join("builds/blink/esp32/out");
    std::fs::create_dir_all(&out).unwrap();
    std::fs::copy(&gpio, out.join("blink.ino.merged.bin")).unwrap();

    let params = qemu_params(&q.pty, json!({ "project": "blink", "board": "esp32" }));
    let token = match c.call("board.flash", params.clone()).await {
        Err(ClientError::Rpc(e)) => {
            assert_eq!(e.code, code::CONSENT_REQUIRED);
            let data = e.data.unwrap();
            assert_valid(
                &method_def("board.flash")["errorData"]["-32002"],
                &data,
                "flash consent",
            );
            data["token"].as_str().unwrap().to_owned()
        }
        other => panic!("flashed without consent: {other:?}"),
    };
    let mut with_token = params.clone();
    with_token["consent"] = json!(token);
    let r = c.call("board.flash", with_token).await.unwrap();
    let (progress, done) = board_events(&mut c, &r["job"]).await;
    assert_eq!(done["ok"], true, "{done}");
    assert!(progress.iter().any(|p| p["stage"] == "writing"));
    let written = done["written"].as_u64().unwrap() as usize;
    drop(q); // let QEMU flush the flash file
    let image = std::fs::read(&gpio).unwrap();
    let on_disk = std::fs::read(&flash).unwrap();
    assert_eq!(
        &on_disk[..written],
        &image[..written],
        "the build did not reach the flash"
    );
}

#[tokio::test]
async fn board_flash_refuses_a_build_for_another_chip() {
    let (Some(c3), Some(gpio)) = (firmware("hello-esp32c3.bin"), firmware("gpio-esp32.bin")) else {
        eprintln!("skipping: test firmware not built");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let flash = dir.path().join("flash.bin");
    std::fs::copy(c3, &flash).unwrap();
    let Some(q) = download_mode_qemu("esp32c3", &flash, &dir.path().join("q.log")) else {
        eprintln!("skipping: QEMU not installed");
        return;
    };
    let (d, sock, _lock) = serial_daemon(&q.pty).await;
    let mut c = Client::connect(&sock).await.unwrap();
    with_library(&mut c).await;
    let out = d.path().join("builds/blink/esp32/out");
    std::fs::create_dir_all(&out).unwrap();
    std::fs::copy(&gpio, out.join("blink.ino.merged.bin")).unwrap();
    let params = qemu_params(&q.pty, json!({ "project": "blink", "board": "esp32" }));
    let token = match c.call("board.flash", params.clone()).await {
        Err(ClientError::Rpc(e)) => e.data.unwrap()["token"].as_str().unwrap().to_owned(),
        other => panic!("expected consent required, got {other:?}"),
    };
    let mut with_token = params;
    with_token["consent"] = json!(token);
    let r = c.call("board.flash", with_token).await.unwrap();
    let (_, done) = board_events(&mut c, &r["job"]).await;
    assert_eq!(done["ok"], false);
    assert!(
        done["error"].as_str().unwrap().contains("esp32c3"),
        "{done}"
    );
}

#[tokio::test]
async fn board_erase_needs_confirmation_and_the_stub() {
    let Some(hello) = firmware("hello-esp32.bin") else {
        eprintln!("skipping: hello firmware not built");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let flash = dir.path().join("flash.bin");
    std::fs::copy(hello, &flash).unwrap();
    let Some(q) = download_mode_qemu("esp32", &flash, &dir.path().join("q.log")) else {
        eprintln!("skipping: QEMU not installed");
        return;
    };
    let (_d, sock, _lock) = serial_daemon(&q.pty).await;
    let mut c = Client::connect(&sock).await.unwrap();
    match c
        .call(
            "board.erase",
            qemu_params(&q.pty, json!({ "confirm": false })),
        )
        .await
    {
        Err(ClientError::Rpc(e)) => assert_eq!(e.code, code::INVALID_PARAMS),
        other => panic!("erased without confirmation: {other:?}"),
    }
    // Erasing is a stub-only command; QEMU cannot run the stub, so it must fail
    // cleanly and quickly rather than hang. (Real boards use the stub by default.)
    let r = c
        .call(
            "board.erase",
            qemu_params(&q.pty, json!({ "confirm": true })),
        )
        .await
        .unwrap();
    let (_, done) = board_events(&mut c, &r["job"]).await;
    assert_eq!(done["ok"], false);
    assert!(done["error"].as_str().unwrap().contains("stub"), "{done}");
    drop(q);
    assert_eq!(
        std::fs::read(&flash).unwrap()[0x1000],
        0xE9,
        "flash was changed"
    );
}
