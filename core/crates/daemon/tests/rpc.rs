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
    s.circuits_root = dir.join("circuits");
    Arc::new(s)
}

/// A stand-in arduino-cli: installs nothing, and "compiles" by printing a line and
/// writing empty output files where arduino-cli would.
fn fake_arduino_cli(dir: &Path) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join("fake-arduino-cli");
    write_script(
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
    );
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
    let params = json!({ "client": "test", "protocol": "0.9.0" });
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
    // A child that a parallel test forks keeps a copy of the lock's descriptor until
    // it execs (close-on-exec), so allow a moment before the lock is free again.
    for _ in 0..40 {
        if esp32bench_daemon::bind(&sock).is_ok() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("the lock was not released after the first daemon stopped");
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
    // Generous: under a full parallel run, QEMU tests load the machine.
    loop {
        let n = tokio::time::timeout(std::time::Duration::from_secs(15), c.next_notification())
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
            "qemu-lcgamboa",
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

/// Wait for sim.output until `marker` has appeared; checks each event's schema.
async fn sim_wait_for(c: &mut Client, sub: &Value, marker: &str) {
    let mut text = String::new();
    while !text.contains(marker) {
        let n = tokio::time::timeout(std::time::Duration::from_secs(30), c.next_notification())
            .await
            .unwrap_or_else(|_| panic!("no `{marker}` in: {text}"))
            .unwrap();
        let event = n["method"].as_str().unwrap();
        assert_valid(&method_def(event)["params"], &n["params"], event);
        assert_eq!(event, "sim.output", "{n}");
        assert_eq!(&n["params"]["subscription"], sub);
        text.push_str(n["params"]["text"].as_str().unwrap());
    }
}

/// The engine sim.start should pick here: the fork once it is built (and the host
/// binary sits next to the tests), else Espressif's QEMU.
fn expected_engine() -> &'static str {
    match esp32bench_sim::engine::launch(esp32bench_sim::Chip::Esp32, None) {
        Ok(l) => l.engine.as_str(),
        Err(_) => "espressif",
    }
}

fn qemu_processes() -> usize {
    std::fs::read_dir("/proc")
        .unwrap()
        .filter_map(|e| std::fs::read_to_string(e.ok()?.path().join("cmdline")).ok())
        .filter(|c| {
            (c.contains("qemu-system") || c.contains("esp32bench-qemu-host"))
                && c.contains("esp32bench")
        })
        .count()
}

#[tokio::test]
async fn sim_runs_a_build_with_console_and_reset() {
    let Some(hello) = firmware("hello-esp32.bin") else {
        eprintln!("skipping: hello firmware not built");
        return;
    };
    if esp32bench_sim::install::find_qemu(esp32bench_sim::Arch::Xtensa).is_err() {
        eprintln!("skipping: QEMU not installed");
        return;
    }
    let (d, sock, _lock) = daemon().await;
    let mut c = Client::connect(&sock).await.unwrap();
    with_library(&mut c).await;
    let out = d.path().join("builds/blink/esp32/out");
    std::fs::create_dir_all(&out).unwrap();
    std::fs::copy(&hello, out.join("blink.ino.merged.bin")).unwrap();

    let r = c
        .call("sim.start", json!({ "project": "blink", "board": "esp32" }))
        .await
        .unwrap();
    assert_valid(&method_def("sim.start")["result"], &r, "sim.start result");
    assert_eq!(r["chip"], "esp32");
    assert_eq!(r["engine"], expected_engine());
    eprintln!("engine: {}", r["engine"]);
    let sub = r["subscription"].clone();
    sim_wait_for(&mut c, &sub, "esp32bench-hello ready").await;

    use base64::Engine;
    let data = base64::engine::general_purpose::STANDARD.encode(b"ping 42\r\n");
    c.call("sim.write", json!({ "subscription": sub, "data": data }))
        .await
        .unwrap();
    sim_wait_for(&mut c, &sub, "esp32bench-hello echo ping 42").await;

    c.call("sim.reset", json!({ "subscription": sub }))
        .await
        .unwrap();
    sim_wait_for(&mut c, &sub, "esp32bench-hello ready").await;

    let before = qemu_processes();
    assert!(before >= 1);
    c.call("core.unsubscribe", json!({ "subscription": sub }))
        .await
        .unwrap();
    for _ in 0..50 {
        if qemu_processes() < before {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(
        qemu_processes() < before,
        "QEMU still running after unsubscribe"
    );
    match c.call("sim.reset", json!({ "subscription": sub })).await {
        Err(ClientError::Rpc(e)) => assert_eq!(e.code, code::NOT_FOUND),
        other => panic!("reset a stopped simulator: {other:?}"),
    }
}

/// Wait for a debug event named `event`, checking every notification on the way
/// against its schema (UART output keeps arriving meanwhile).
async fn debug_wait(c: &mut Client, sub: &Value, event: &str) -> Value {
    loop {
        let n = tokio::time::timeout(std::time::Duration::from_secs(30), c.next_notification())
            .await
            .unwrap_or_else(|_| panic!("no {event}"))
            .unwrap();
        let method = n["method"].as_str().unwrap();
        assert_valid(&method_def(method)["params"], &n["params"], method);
        assert_eq!(&n["params"]["subscription"], sub);
        if method == event {
            return n["params"].clone();
        }
        assert_ne!(method, "debug.exited", "GDB exited");
    }
}

#[tokio::test]
async fn sim_debugs_a_build_with_gdb() {
    let (Some(hello), Some(elf)) = (
        firmware("hello-esp32.bin"),
        Some(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../../tests/firmware/build/hello-esp32/esp32bench-hello.elf"),
        )
        .filter(|p| p.is_file()),
    ) else {
        eprintln!("skipping: hello firmware not built");
        return;
    };
    let data = esp32bench_toolchain::Env::default_arduino_data().unwrap_or_default();
    if esp32bench_sim::install::find_qemu(esp32bench_sim::Arch::Xtensa).is_err()
        || esp32bench_debug::find_gdb(&data, "esp32").is_none()
    {
        eprintln!("skipping: needs QEMU and the Arduino core's GDB");
        return;
    }
    let (d, sock, _lock) = daemon().await;
    let mut c = Client::connect(&sock).await.unwrap();
    with_library(&mut c).await;
    let out = d.path().join("builds/blink/esp32/out");
    std::fs::create_dir_all(&out).unwrap();
    std::fs::copy(&hello, out.join("blink.ino.merged.bin")).unwrap();
    std::fs::copy(&elf, out.join("blink.ino.elf")).unwrap();

    let r = c
        .call(
            "sim.start",
            json!({ "project": "blink", "board": "esp32", "debug": true }),
        )
        .await
        .unwrap();
    assert_valid(&method_def("sim.start")["result"], &r, "sim.start result");
    assert_eq!(r["debug"], true);
    assert_eq!(r["engine"], expected_engine());
    eprintln!("engine: {}", r["engine"]);
    let sub = r["subscription"].clone();
    // Starts paused at the reset vector.
    debug_wait(&mut c, &sub, "debug.stopped").await;

    let r = c
        .call(
            "debug.break",
            json!({ "subscription": sub, "location": "app_main" }),
        )
        .await
        .unwrap();
    assert_valid(
        &method_def("debug.break")["result"],
        &r,
        "debug.break result",
    );
    assert_eq!(r["breakpoint"]["func"], "app_main");
    let number = r["breakpoint"]["number"].clone();

    c.call(
        "debug.control",
        json!({ "subscription": sub, "action": "continue" }),
    )
    .await
    .unwrap();
    debug_wait(&mut c, &sub, "debug.running").await;
    let stop = debug_wait(&mut c, &sub, "debug.stopped").await;
    assert_eq!(stop["reason"], "breakpoint-hit");
    assert_eq!(stop["breakpoint"], number);
    assert_eq!(stop["frame"]["func"], "app_main");
    assert!(stop["frame"]["file"].as_str().unwrap().ends_with("hello.c"));

    let r = c
        .call("debug.inspect", json!({ "subscription": sub }))
        .await
        .unwrap();
    assert_valid(
        &method_def("debug.inspect")["result"],
        &r,
        "debug.inspect result",
    );
    assert_eq!(r["frames"][0]["func"], "app_main");
    assert!(
        r["registers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x["name"] == "pc")
    );

    c.call(
        "debug.control",
        json!({ "subscription": sub, "action": "next" }),
    )
    .await
    .unwrap();
    let stop = debug_wait(&mut c, &sub, "debug.stopped").await;
    assert_eq!(stop["reason"], "end-stepping-range");

    c.call(
        "debug.delete",
        json!({ "subscription": sub, "breakpoint": number }),
    )
    .await
    .unwrap();
    c.call(
        "debug.control",
        json!({ "subscription": sub, "action": "continue" }),
    )
    .await
    .unwrap();
    // It runs on: the firmware gets to its ready line.
    let mut seen = String::new();
    while !seen.contains("esp32bench-hello ready") {
        let n = tokio::time::timeout(std::time::Duration::from_secs(30), c.next_notification())
            .await
            .unwrap()
            .unwrap();
        if n["method"] == "sim.output" {
            seen.push_str(n["params"]["text"].as_str().unwrap());
        }
    }
    c.call(
        "debug.control",
        json!({ "subscription": sub, "action": "interrupt" }),
    )
    .await
    .unwrap();
    let stop = debug_wait(&mut c, &sub, "debug.stopped").await;
    assert_eq!(stop["reason"], "signal-received");

    // Locations are checked before GDB sees them.
    match c
        .call(
            "debug.break",
            json!({ "subscription": sub, "location": "x\n-gdb-exit" }),
        )
        .await
    {
        Err(ClientError::Rpc(e)) => assert_eq!(e.code, code::INVALID_PARAMS),
        other => panic!("odd location accepted: {other:?}"),
    }
    c.call("core.unsubscribe", json!({ "subscription": sub }))
        .await
        .unwrap();
}

#[tokio::test]
async fn debug_needs_a_debug_session() {
    let Some(hello) = firmware("hello-esp32.bin") else {
        eprintln!("skipping: hello firmware not built");
        return;
    };
    if esp32bench_sim::install::find_qemu(esp32bench_sim::Arch::Xtensa).is_err() {
        eprintln!("skipping: QEMU not installed");
        return;
    }
    let (d, sock, _lock) = daemon().await;
    let mut c = Client::connect(&sock).await.unwrap();
    with_library(&mut c).await;
    let out = d.path().join("builds/blink/esp32/out");
    std::fs::create_dir_all(&out).unwrap();
    std::fs::copy(&hello, out.join("blink.ino.merged.bin")).unwrap();
    // No ELF: debugging is refused up front.
    match c
        .call(
            "sim.start",
            json!({ "project": "blink", "board": "esp32", "debug": true }),
        )
        .await
    {
        Err(ClientError::Rpc(e)) => assert_eq!(e.code, code::NOT_FOUND),
        other => panic!("debugged without an ELF: {other:?}"),
    }
    let r = c
        .call("sim.start", json!({ "project": "blink", "board": "esp32" }))
        .await
        .unwrap();
    assert_eq!(r["debug"], false);
    match c
        .call(
            "debug.control",
            json!({ "subscription": r["subscription"], "action": "continue" }),
        )
        .await
    {
        Err(ClientError::Rpc(e)) => assert_eq!(e.code, code::FAILED),
        other => panic!("debug on a plain simulator: {other:?}"),
    }
    match c
        .call("debug.inspect", json!({ "subscription": 999 }))
        .await
    {
        Err(ClientError::Rpc(e)) => assert_eq!(e.code, code::NOT_FOUND),
        other => panic!("debug on no simulator: {other:?}"),
    }
}

#[tokio::test]
async fn sim_sends_the_virtual_panel() {
    let Some(rgb) = firmware("rgb-esp32.bin") else {
        eprintln!("skipping: rgb firmware not built (tests/firmware/build.sh rgb)");
        return;
    };
    if esp32bench_sim::engine::launch(esp32bench_sim::Chip::Esp32, None).is_err() {
        eprintln!("skipping: QEMU not installed");
        return;
    }
    let (d, sock, _lock) = daemon().await;
    let mut c = Client::connect(&sock).await.unwrap();
    with_library(&mut c).await;
    let out = d.path().join("builds/blink/esp32/out");
    std::fs::create_dir_all(&out).unwrap();
    std::fs::copy(&rgb, out.join("blink.ino.merged.bin")).unwrap();
    let r = c
        .call("sim.start", json!({ "project": "blink", "board": "esp32" }))
        .await
        .unwrap();
    let sub = r["subscription"].clone();
    use base64::Engine;
    // Wait for the full four-quadrant picture.
    loop {
        let n = tokio::time::timeout(std::time::Duration::from_secs(30), c.next_notification())
            .await
            .expect("no picture with all four quadrants")
            .unwrap();
        let method = n["method"].as_str().unwrap();
        assert_valid(&method_def(method)["params"], &n["params"], method);
        assert_eq!(n["params"]["subscription"], sub);
        if method != "sim.frame" {
            continue;
        }
        let p = &n["params"];
        let (w, h) = (p["width"].as_u64().unwrap(), p["height"].as_u64().unwrap());
        assert_eq!((w, h), (320, 240));
        let rgb = base64::engine::general_purpose::STANDARD
            .decode(p["data"].as_str().unwrap())
            .unwrap();
        assert_eq!(rgb.len() as u64, w * h * 3);
        let px = |x: u64, y: u64| {
            let i = usize::try_from((y * w + x) * 3).unwrap();
            [rgb[i], rgb[i + 1], rgb[i + 2]]
        };
        if px(250, 200) == [255, 255, 255] {
            assert_eq!(px(250, 10), [0, 255, 0]);
            assert_eq!(px(10, 200), [0, 0, 255]);
            break;
        }
    }
    c.call("core.unsubscribe", json!({ "subscription": sub }))
        .await
        .unwrap();
}

#[tokio::test]
async fn sim_bridges_the_projects_pins() {
    let Some(gpio_fw) = firmware("gpio-esp32.bin") else {
        eprintln!("skipping: gpio firmware not built (tests/firmware/build.sh gpio)");
        return;
    };
    let Ok(launch) = esp32bench_sim::engine::launch(esp32bench_sim::Chip::Esp32, None) else {
        eprintln!("skipping: QEMU not installed");
        return;
    };
    // The example library, with Blink's ESP32 build using GPIO 2 and 4 (and GND, which
    // is not a GPIO).
    let dir = tempfile::tempdir().unwrap();
    let mut manifest: Value =
        serde_json::from_str(&std::fs::read_to_string(example_manifest()).unwrap()).unwrap();
    for b in manifest["projects"][0]["builds"].as_array_mut().unwrap() {
        if b["board"] == "esp32" {
            b["pins"] = json!(["2", "4", "GND"]);
        }
    }
    let source = dir.path().join("manifest.json");
    std::fs::write(&source, manifest.to_string()).unwrap();
    let (d, sock, _lock) = daemon_with_source(&source).await;
    let mut c = Client::connect(&sock).await.unwrap();
    with_library(&mut c).await;
    let out = d.path().join("builds/blink/esp32/out");
    std::fs::create_dir_all(&out).unwrap();
    std::fs::copy(&gpio_fw, out.join("blink.ino.merged.bin")).unwrap();

    let r = c
        .call("sim.start", json!({ "project": "blink", "board": "esp32" }))
        .await
        .unwrap();
    assert_valid(&method_def("sim.start")["result"], &r, "sim.start result");
    let sub = r["subscription"].clone();
    if !launch.hooks {
        // Espressif's QEMU (the fork is not built): no pin access, and it says so.
        assert_eq!(r["pins"], json!([]));
        eprintln!("skipping the pin checks: no fork build here");
        return;
    }
    assert_eq!(r["pins"], json!([2, 4]));

    // GPIO 2 toggles: seen high and low, as an output.
    let (mut high, mut low) = (false, false);
    let mut text = String::new();
    while !(high && low) {
        let n = tokio::time::timeout(std::time::Duration::from_secs(20), c.next_notification())
            .await
            .expect("GPIO 2 did not toggle")
            .unwrap();
        let method = n["method"].as_str().unwrap();
        assert_valid(&method_def(method)["params"], &n["params"], method);
        if method == "sim.output" {
            text.push_str(n["params"]["text"].as_str().unwrap());
        }
        if method != "sim.pins" {
            continue;
        }
        for pin in n["params"]["pins"].as_array().unwrap() {
            if pin["gpio"] == 2 && pin["level"] == true {
                high = true;
            }
            if pin["gpio"] == 2 && pin["level"] == false {
                low = true;
            }
        }
    }
    // Drive GPIO 4; the firmware reports what it reads.
    c.call(
        "sim.setPin",
        json!({ "subscription": sub, "gpio": 4, "level": true }),
    )
    .await
    .unwrap();
    while !text.contains("esp32bench-gpio in 4=1") {
        let n = tokio::time::timeout(std::time::Duration::from_secs(10), c.next_notification())
            .await
            .unwrap_or_else(|_| panic!("firmware never read GPIO 4 high: {text}"))
            .unwrap();
        if n["method"] == "sim.output" {
            text.push_str(n["params"]["text"].as_str().unwrap());
        }
    }
    // Only the project's pins.
    match c
        .call(
            "sim.setPin",
            json!({ "subscription": sub, "gpio": 9, "level": true }),
        )
        .await
    {
        Err(ClientError::Rpc(e)) => assert_eq!(e.code, code::FAILED),
        other => panic!("drove a pin the project does not use: {other:?}"),
    }
    c.call("core.unsubscribe", json!({ "subscription": sub }))
        .await
        .unwrap();
}

#[tokio::test]
async fn circuit_round_trip() {
    let (_d, sock, _lock) = daemon().await;
    let mut c = Client::connect(&sock).await.unwrap();
    let r = c
        .call(
            "circuit.get",
            json!({ "project": "blink", "board": "esp32" }),
        )
        .await
        .unwrap();
    assert_valid(
        &method_def("circuit.get")["result"],
        &r,
        "circuit.get result",
    );
    assert_eq!(r["circuit"]["parts"], json!([]));
    let circuit = json!({ "schemaVersion": "1.0.0", "parts": [
        { "id": "led", "kind": "led", "pin": 2, "label": "Status LED" },
        { "id": "go", "kind": "button", "pin": 4 }
    ]});
    let params = json!({ "project": "blink", "board": "esp32", "circuit": circuit });
    assert_valid(
        &method_def("circuit.set")["params"],
        &params,
        "circuit.set params",
    );
    c.call("circuit.set", params).await.unwrap();
    let r = c
        .call(
            "circuit.get",
            json!({ "project": "blink", "board": "esp32" }),
        )
        .await
        .unwrap();
    assert_eq!(r["circuit"]["parts"][0]["label"], "Status LED");
    assert_eq!(r["circuit"]["parts"][1]["kind"], "button");
    // Checked before saving.
    for bad in [
        json!({ "project": "blink", "board": "esp32", "circuit": { "schemaVersion": "1.0.0", "parts": [
            { "id": "a", "kind": "button", "pin": 4 }, { "id": "b", "kind": "button", "pin": 4 } ] } }),
        json!({ "project": "../x", "board": "esp32", "circuit": { "schemaVersion": "1.0.0", "parts": [] } }),
        json!({ "project": "blink", "board": "esp32", "circuit": { "schemaVersion": "1.0.0", "parts": [
            { "id": "a", "kind": "motor", "pin": 4 } ] } }),
    ] {
        match c.call("circuit.set", bad.clone()).await {
            Err(ClientError::Rpc(e)) => assert_eq!(e.code, code::INVALID_PARAMS, "{bad}"),
            other => panic!("saved a bad circuit {bad}: {other:?}"),
        }
    }
}

#[tokio::test]
async fn sim_runs_the_boards_parts() {
    let Some(gpio_fw) = firmware("gpio-esp32.bin") else {
        eprintln!("skipping: gpio firmware not built (tests/firmware/build.sh gpio)");
        return;
    };
    match esp32bench_sim::engine::launch(esp32bench_sim::Chip::Esp32, None) {
        Ok(l) if l.hooks => {}
        _ => {
            eprintln!("skipping: needs lcgamboa's QEMU library");
            return;
        }
    }
    let (d, sock, _lock) = daemon().await;
    let mut c = Client::connect(&sock).await.unwrap();
    with_library(&mut c).await;
    let out = d.path().join("builds/blink/esp32/out");
    std::fs::create_dir_all(&out).unwrap();
    std::fs::copy(&gpio_fw, out.join("blink.ino.merged.bin")).unwrap();
    // An LED on the firmware's output, a button (to 3V3, so active high) on its input.
    // GPIO 4 is not one of Blink's pins: the circuit adds it.
    c.call(
        "circuit.set",
        json!({ "project": "blink", "board": "esp32", "circuit": {
        "schemaVersion": "1.0.0", "parts": [
            { "id": "led", "kind": "led", "pin": 2 },
            { "id": "go", "kind": "button", "pin": 4 },
            { "id": "air", "kind": "bme280", "i2c": { "bus": 0, "address": 118 } }
        ] } }),
    )
    .await
    .unwrap();
    let r = c
        .call("sim.start", json!({ "project": "blink", "board": "esp32" }))
        .await
        .unwrap();
    assert_valid(&method_def("sim.start")["result"], &r, "sim.start result");
    assert_eq!(r["pins"], json!([2, 4]));
    assert_eq!(r["partsActive"], true);
    let sub = r["subscription"].clone();

    let mut text = String::new();
    let mut led = Vec::new();
    // Wait until the LED has been seen on and off.
    while !(led.contains(&true) && led.contains(&false) && led.len() >= 3) {
        let n = tokio::time::timeout(std::time::Duration::from_secs(20), c.next_notification())
            .await
            .expect("the LED did not blink")
            .unwrap();
        let method = n["method"].as_str().unwrap();
        assert_valid(&method_def(method)["params"], &n["params"], method);
        match method {
            "sim.output" => text.push_str(n["params"]["text"].as_str().unwrap()),
            "sim.parts" => {
                for p in n["params"]["parts"].as_array().unwrap() {
                    if p["id"] == "led" {
                        led.push(p["on"].as_bool().unwrap());
                    }
                }
            }
            _ => {}
        }
    }
    // Released, the button holds GPIO 4 low; pressed, high.
    assert!(text.contains("esp32bench-gpio in 4=0"), "{text}");
    let press = json!({ "subscription": sub, "part": "go", "control": "press", "value": true });
    assert_valid(
        &method_def("sim.control")["params"],
        &press,
        "sim.control params",
    );
    c.call("sim.control", press).await.unwrap();
    let mut pressed_seen = false;
    while !(text.contains("esp32bench-gpio in 4=1") && pressed_seen) {
        let n = tokio::time::timeout(std::time::Duration::from_secs(10), c.next_notification())
            .await
            .unwrap_or_else(|_| panic!("the press did not reach the firmware: {text}"))
            .unwrap();
        match n["method"].as_str().unwrap() {
            "sim.output" => text.push_str(n["params"]["text"].as_str().unwrap()),
            "sim.parts" => {
                pressed_seen |= n["params"]["parts"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|p| p["id"] == "go" && p["on"] == true);
            }
            _ => {}
        }
    }
    // An LED has nothing to press: the part reports the refusal as its error.
    c.call(
        "sim.control",
        json!({ "subscription": sub, "part": "led", "control": "press", "value": true }),
    )
    .await
    .unwrap();
    loop {
        let n = tokio::time::timeout(std::time::Duration::from_secs(10), c.next_notification())
            .await
            .expect("no error for pressing an LED")
            .unwrap();
        let method = n["method"].as_str().unwrap();
        assert_valid(&method_def(method)["params"], &n["params"], method);
        if method == "sim.parts"
            && n["params"]["parts"]
                .as_array()
                .unwrap()
                .iter()
                .any(|p| p["id"] == "led" && p["error"].is_string())
        {
            break;
        }
    }
    // A slider moved during the run is remembered in the saved circuit (1.2
    // `controls`), so the sensor starts there next run; a button press is not.
    c.call(
        "sim.control",
        json!({ "subscription": sub, "part": "air", "control": "temperature", "value": 30.5 }),
    )
    .await
    .unwrap();
    let r = c
        .call(
            "circuit.get",
            json!({ "project": "blink", "board": "esp32" }),
        )
        .await
        .unwrap();
    assert_valid(
        &method_def("circuit.get")["result"],
        &r,
        "circuit.get result",
    );
    let parts = r["circuit"]["parts"].as_array().unwrap();
    let air = parts.iter().find(|p| p["id"] == "air").unwrap();
    assert_eq!(air["controls"], json!({ "temperature": 30.5 }));
    assert!(
        parts
            .iter()
            .all(|p| p["id"] == "air" || p.get("controls").is_none())
    );
    assert_eq!(r["circuit"]["schemaVersion"], "1.2.0");
    // Values are booleans or numbers, nothing else.
    match c
        .call(
            "sim.control",
            json!({ "subscription": sub, "part": "go", "control": "press", "value": "yes" }),
        )
        .await
    {
        Err(ClientError::Rpc(e)) => assert_eq!(e.code, code::INVALID_PARAMS),
        other => panic!("took a string value: {other:?}"),
    }
    c.call("core.unsubscribe", json!({ "subscription": sub }))
        .await
        .unwrap();
}

#[tokio::test]
async fn sim_refuses_workbench_only_boards_and_missing_builds() {
    // The example library plus a C6 build of blink: C6 is not simulated.
    let dir = tempfile::tempdir().unwrap();
    let mut m: Value = serde_json::from_slice(&std::fs::read(example_manifest()).unwrap()).unwrap();
    let mut c6 = m["projects"][0]["builds"][0].clone();
    c6["board"] = json!("esp32c6");
    m["projects"][0]["builds"].as_array_mut().unwrap().push(c6);
    let src = dir.path().join("site.json");
    std::fs::write(&src, serde_json::to_vec(&m).unwrap()).unwrap();
    let (d, sock, _lock) = daemon_with_source(&src).await;
    let mut c = Client::connect(&sock).await.unwrap();
    with_library(&mut c).await;

    let out = d.path().join("builds/blink/esp32c6/out");
    std::fs::create_dir_all(&out).unwrap();
    std::fs::write(out.join("blink.ino.merged.bin"), b"x").unwrap();
    match c
        .call(
            "sim.start",
            json!({ "project": "blink", "board": "esp32c6" }),
        )
        .await
    {
        Err(ClientError::Rpc(e)) => {
            assert_eq!(e.code, code::FAILED);
            assert!(e.message.contains("workbench-only"), "{}", e.message);
        }
        other => panic!("simulated a C6: {other:?}"),
    }
    match c
        .call("sim.start", json!({ "project": "blink", "board": "esp32" }))
        .await
    {
        Err(ClientError::Rpc(e)) => assert_eq!(e.code, code::NOT_FOUND, "{}", e.message),
        other => panic!("started without a build: {other:?}"),
    }
}

/// Write an executable script without this process ever holding it open for writing.
/// Parallel tests fork; a child that inherited a write handle would make running the
/// script fail with "Text file busy" (ETXTBSY).
fn write_script(path: &std::path::Path, content: impl AsRef<[u8]>) {
    use std::io::Write;
    let mut child = std::process::Command::new("sh")
        .args(["-c", "cat > \"$1\" && chmod 755 \"$1\"", "sh"])
        .arg(path)
        .stdin(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(content.as_ref())
        .unwrap();
    assert!(child.wait().unwrap().success());
}

#[tokio::test]
async fn link_open_names_a_library_project_and_nothing_more() {
    let (_d, sock, _lock) = daemon().await;
    let mut c = Client::connect(&sock).await.unwrap();
    let open = |url: &str| json!({ "url": url });
    // Before the library is downloaded, no project can be found.
    match c
        .call("link.open", open("esp32bench://project/blink"))
        .await
    {
        Err(ClientError::Rpc(e)) => assert_eq!(e.code, code::NOT_FOUND),
        other => panic!("expected not found, got {other:?}"),
    }
    c.call("library.refresh", json!({})).await.unwrap();
    c.next_notification().await.unwrap();

    let params = open("esp32bench://project/blink?board=esp32-doit");
    assert_valid(
        &method_def("link.open")["params"],
        &params,
        "link.open params",
    );
    let r = c.call("link.open", params).await.unwrap();
    assert_valid(&method_def("link.open")["result"], &r, "link.open result");
    assert_eq!(r, json!({ "project": "blink", "board": "esp32-doit" }));
    // A board the project has no build for is dropped, not an error.
    let r = c
        .call(
            "link.open",
            open("esp32bench://project/blink?board=esp32c6"),
        )
        .await
        .unwrap();
    assert_eq!(r, json!({ "project": "blink", "board": null }));
    match c.call("link.open", open("esp32bench://project/nope")).await {
        Err(ClientError::Rpc(e)) => assert_eq!(e.code, code::NOT_FOUND),
        other => panic!("expected not found, got {other:?}"),
    }
    // Links are untrusted: anything but the plain form is refused.
    for bad in [
        "esp32bench://project/blink?board=esp32&flash=1",
        "esp32bench://project/../blink",
        "file:///etc/passwd",
    ] {
        match c.call("link.open", open(bad)).await {
            Err(ClientError::Rpc(e)) => assert_eq!(e.code, code::INVALID_PARAMS, "{bad}"),
            other => panic!("{bad}: expected invalid params, got {other:?}"),
        }
    }
}
