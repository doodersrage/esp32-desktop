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
    s.local = esp32bench_build::idf::Registry::new(dir.join("local-projects.json"));
    s.starred = esp32bench_library::starred::Starred::new(dir.join("starred.json"));
    s.known_boards = esp32bench_device::Known::new(dir.join("boards.json"));
    s.commands = esp32bench_library::commands::Commands::new(dir.join("commands.json"));
    // Never the user's own data directory.
    s.breakpoints = esp32bench_library::commands::Commands::new(dir.join("breakpoints.json"));
    s.watches = esp32bench_library::commands::Commands::new(dir.join("watches.json"));
    s.notes = esp32bench_library::notes::Notes::new(dir.join("notes.json"));
    s.sim_data = dir.join("qemu");
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
    if [ -e "$0.fail" ]; then
      echo "/tmp/x/blink/blink.ino: In function 'void loop()':" >&2
      echo "/tmp/x/blink/blink.ino:12:3: error: 'ledd' was not declared in this scope" >&2
      exit 1
    fi
    out=""; build=""; prev=""
    for a in "$@"; do [ "$prev" = "--output-dir" ] && out="$a"; [ "$prev" = "--build-path" ] && build="$a"; prev="$a"; last="$a"; done
    name=$(basename "$last")
    # A compilation database naming the working copy, as arduino-cli writes one.
    [ -n "$build" ] && mkdir -p "$build" && printf '[{"directory":"/x","file":"%s/sketch/%s.ino.cpp","arguments":["g++","-c","%s/sketch/%s.ino.cpp"]}]' "$build" "$name" "$build" "$name" > "$build/compile_commands.json"
    echo "Sketch uses 1234 bytes (1%) of program storage space. Maximum is 1310720 bytes."
    echo "Global variables use 2048 bytes (0%) of dynamic memory, leaving 325632 bytes for local variables. Maximum is 327680 bytes."
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
    let advice = &method_def("device.list")["result"]["properties"]["advice"];
    assert_eq!(
        &method_def("device.watch")["result"]["properties"]["advice"],
        advice
    );
    assert_eq!(
        &method_def("device.changed")["params"]["properties"]["advice"],
        advice
    );
    // Every kind of advice the core gives fits it.
    use esp32bench_device::{Advice, Bridge};
    for a in [
        Advice::DataCable,
        Advice::InstallDriver,
        Advice::BridgeWithoutPort {
            bridge: Bridge::Ch340,
        },
        Advice::SerialAccess,
    ] {
        let v = json!([a]);
        assert_valid(advice, &v, "advice");
    }
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
        assert!(r["advice"].is_array(), "{r}");
    }
    match c.call("device.list", json!({ "all": "yes" })).await {
        Err(ClientError::Rpc(e)) => assert_eq!(e.code, code::INVALID_PARAMS),
        other => panic!("expected invalid params, got {other:?}"),
    }
}

// The Unix lock file and socket; on Windows the pipe is the lock.
#[cfg(unix)]
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

#[cfg(unix)]
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
async fn library_says_a_newer_app_is_out_and_keeps_stars() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("site.json");
    let mut m: Value = serde_json::from_slice(&std::fs::read(example_manifest()).unwrap()).unwrap();
    m["desktop"]["version"] = json!("99.0.0");
    std::fs::write(&src, serde_json::to_vec(&m).unwrap()).unwrap();
    let (_d, sock, _lock) = daemon_with_source(&src).await;
    let mut c = Client::connect(&sock).await.unwrap();
    c.call("library.refresh", json!({})).await.unwrap();
    c.next_notification().await.unwrap();

    let def = method_def("library.star");
    let params = json!({ "id": "blink", "starred": true });
    assert_valid(&def["params"], &params, "library.star params");
    let r = c.call("library.star", params).await.unwrap();
    assert_valid(&def["result"], &r, "library.star result");
    assert_eq!(r["starred"], json!(["blink"]));

    let list = c.call("library.list", json!({})).await.unwrap();
    assert_valid(&method_def("library.list")["result"], &list, "library.list");
    assert_eq!(list["update"]["version"], "99.0.0");
    assert_eq!(list["update"]["url"], "https://esp32bench.dev/desktop/");
    for p in list["projects"].as_array().unwrap() {
        assert_eq!(p["starred"], p["id"] == "blink", "{p}");
    }

    let r = c
        .call("library.star", json!({ "id": "blink", "starred": false }))
        .await
        .unwrap();
    assert_eq!(r["starred"], json!([]));
    match c
        .call("library.star", json!({ "id": "", "starred": true }))
        .await
    {
        Err(ClientError::Rpc(e)) => assert_eq!(e.code, code::INVALID_PARAMS),
        other => panic!("expected invalid params, got {other:?}"),
    }

    // The example's own release is this core's or older: no notice.
    std::fs::copy(example_manifest(), &src).unwrap();
    c.call("library.refresh", json!({})).await.unwrap();
    c.next_notification().await.unwrap();
    let list = c.call("library.list", json!({})).await.unwrap();
    assert!(list.get("update").is_none(), "{}", list["update"]);
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
    // What each pin is wired to, per board.
    assert_eq!(r["pinLabels"]["esp32"]["2"], "LED anode");
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
    let status = json!({ "project": "blink", "board": "esp32" });
    assert_valid(
        &method_def("build.status")["params"],
        &status,
        "build.status params",
    );
    let before = c.call("build.status", status.clone()).await.unwrap();
    assert_valid(
        &method_def("build.status")["result"],
        &before,
        "build.status result",
    );
    assert_eq!(before, json!({ "built": false }));

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
        assert_eq!(
            n["params"]["size"],
            json!({ "flash": 1234, "flashMax": 1310720, "ram": 2048, "ramMax": 327680 })
        );
        // How long it took, in tenths of a second.
        let secs = n["params"]["seconds"].as_f64().unwrap();
        assert!((0.0..60.0).contains(&secs), "{n}");
        assert_eq!((secs * 10.0).round(), secs * 10.0, "{n}");
        break;
    }
    assert!(saw_log);

    // The library says which boards it is built for.
    let list = c.call("library.list", json!({})).await.unwrap();
    let blink = list["projects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == "blink")
        .unwrap()
        .clone();
    assert_eq!(blink["built"], json!(["esp32"]));
    // Built, and what the library asks for; another board is not.
    let after = c.call("build.status", status).await.unwrap();
    assert_valid(
        &method_def("build.status")["result"],
        &after,
        "build.status result",
    );
    assert_eq!(after["built"], true);
    assert_eq!(after["current"], true);
    assert!(after["builtAt"].as_u64().unwrap() > 0);
    let other = c
        .call(
            "build.status",
            json!({ "project": "blink", "board": "esp32s3" }),
        )
        .await
        .unwrap();
    assert_eq!(other["built"], false);
    match c
        .call(
            "build.status",
            json!({ "project": "../x", "board": "esp32" }),
        )
        .await
    {
        Err(ClientError::Rpc(e)) => assert_eq!(e.code, code::INVALID_PARAMS),
        other => panic!("expected invalid params, got {other:?}"),
    }
}

#[tokio::test]
async fn frame_save_writes_png() {
    let (_d, sock, _lock) = daemon().await;
    let mut c = Client::connect(&sock).await.unwrap();
    let out = tempfile::tempdir().unwrap();
    let path = out.path().join("panel.png");
    let params =
        json!({ "path": path, "width": 2, "height": 1, "data": b64(&[255, 0, 0, 0, 0, 255]) });
    assert_valid(
        &method_def("frame.save")["params"],
        &params,
        "frame.save params",
    );
    let r = c.call("frame.save", params).await.unwrap();
    assert_valid(&method_def("frame.save")["result"], &r, "frame.save result");
    assert!(std::fs::read(&path).unwrap().starts_with(b"\x89PNG"));
    for bad in [
        json!({ "path": path, "width": 3, "height": 1, "data": b64(&[1, 2, 3]) }),
        json!({ "path": "rel.png", "width": 1, "height": 1, "data": b64(&[1, 2, 3]) }),
    ] {
        match c.call("frame.save", bad).await {
            Err(ClientError::Rpc(e)) => assert_eq!(e.code, code::INVALID_PARAMS),
            other => panic!("expected invalid params, got {other:?}"),
        }
    }
}

#[tokio::test]
async fn plot_save_writes_csv() {
    let (_d, sock, _lock) = daemon().await;
    let mut c = Client::connect(&sock).await.unwrap();
    let out = tempfile::tempdir().unwrap();
    let path = out.path().join("plot.csv");
    let def = method_def("plot.save");
    let params = json!({ "path": path, "rows": [
        [{ "name": "temp", "value": 21.5 }, { "name": "hum", "value": 45 }],
        [{ "name": "temp", "value": 22 }],
    ] });
    assert_valid(&def["params"], &params, "plot.save params");
    let r = c.call("plot.save", params).await.unwrap();
    assert_valid(&def["result"], &r, "plot.save result");
    assert_eq!(r["rows"], 2);
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "row,temp,hum\n1,21.5,45\n2,22,\n"
    );
    match c
        .call("plot.save", json!({ "path": "rel.csv", "rows": [] }))
        .await
    {
        Err(ClientError::Rpc(e)) => assert_eq!(e.code, code::INVALID_PARAMS),
        other => panic!("expected invalid params, got {other:?}"),
    }
}

#[tokio::test]
async fn commands_are_kept_per_project() {
    let (_d, sock, _lock) = daemon().await;
    let mut c = Client::connect(&sock).await.unwrap();
    let get = json!({ "project": "simon" });
    assert_valid(
        &method_def("commands.get")["params"],
        &get,
        "commands.get params",
    );
    let r = c.call("commands.get", get.clone()).await.unwrap();
    assert_valid(
        &method_def("commands.get")["result"],
        &r,
        "commands.get result",
    );
    assert_eq!(r["commands"], json!([]));
    let set = json!({ "project": "simon", "commands": ["help", "", "status"] });
    assert_valid(
        &method_def("commands.set")["params"],
        &set,
        "commands.set params",
    );
    let r = c.call("commands.set", set).await.unwrap();
    assert_valid(
        &method_def("commands.set")["result"],
        &r,
        "commands.set result",
    );
    assert_eq!(r["commands"], json!(["help", "status"]));
    assert_eq!(
        c.call("commands.get", get).await.unwrap()["commands"],
        json!(["help", "status"])
    );
    assert_eq!(
        c.call("commands.get", json!({})).await.unwrap()["commands"],
        json!([])
    );
    match c.call("commands.set", json!({ "project": "simon" })).await {
        Err(ClientError::Rpc(e)) => assert_eq!(e.code, code::INVALID_PARAMS),
        other => panic!("expected invalid params, got {other:?}"),
    }
}

#[tokio::test]
async fn watches_are_kept_per_project() {
    let (_d, sock, _lock) = daemon().await;
    let mut c = Client::connect(&sock).await.unwrap();
    let get = json!({ "project": "blink" });
    assert_valid(
        &method_def("watches.get")["params"],
        &get,
        "watches.get params",
    );
    let r = c.call("watches.get", get.clone()).await.unwrap();
    assert_valid(
        &method_def("watches.get")["result"],
        &r,
        "watches.get result",
    );
    assert_eq!(r["watches"], json!([]));
    let set = json!({ "project": "blink", "watches": ["count * 2", "buf[3]", "count * 2"] });
    assert_valid(
        &method_def("watches.set")["params"],
        &set,
        "watches.set params",
    );
    let r = c.call("watches.set", set).await.unwrap();
    assert_valid(
        &method_def("watches.set")["result"],
        &r,
        "watches.set result",
    );
    assert_eq!(r["watches"], json!(["count * 2", "buf[3]"]));
    assert_eq!(
        c.call("watches.get", get.clone()).await.unwrap()["watches"],
        json!(["count * 2", "buf[3]"])
    );
    assert_eq!(
        c.call("watches.get", json!({ "project": "other" }))
            .await
            .unwrap()["watches"],
        json!([])
    );
    let long = "x".repeat(201);
    for bad in [
        json!({ "project": "blink", "watches": ["$_shell(\"x\")"] }),
        json!({ "project": "blink", "watches": ["a\nb"] }),
        json!({ "project": "blink", "watches": [long] }),
        json!({ "project": "", "watches": [] }),
        json!({ "project": "blink" }),
    ] {
        match c.call("watches.set", bad).await {
            Err(ClientError::Rpc(e)) => assert_eq!(e.code, code::INVALID_PARAMS),
            other => panic!("expected invalid params, got {other:?}"),
        }
    }
    assert_eq!(
        c.call("watches.get", get).await.unwrap()["watches"],
        json!(["count * 2", "buf[3]"])
    );
}

#[tokio::test]
async fn breakpoints_are_kept_per_project() {
    let (_d, sock, _lock) = daemon().await;
    let mut c = Client::connect(&sock).await.unwrap();
    let get = json!({ "project": "blink" });
    assert_valid(
        &method_def("breakpoints.get")["params"],
        &get,
        "breakpoints.get params",
    );
    let r = c.call("breakpoints.get", get.clone()).await.unwrap();
    assert_valid(
        &method_def("breakpoints.get")["result"],
        &r,
        "breakpoints.get result",
    );
    assert_eq!(r["breakpoints"], json!([]));
    let set =
        json!({ "project": "blink", "breakpoints": ["blink.ino:12", "setup", "blink.ino:12"] });
    assert_valid(
        &method_def("breakpoints.set")["params"],
        &set,
        "breakpoints.set params",
    );
    let r = c.call("breakpoints.set", set).await.unwrap();
    assert_valid(
        &method_def("breakpoints.set")["result"],
        &r,
        "breakpoints.set result",
    );
    assert_eq!(r["breakpoints"], json!(["blink.ino:12", "setup"]));
    assert_eq!(
        c.call("breakpoints.get", get.clone()).await.unwrap()["breakpoints"],
        json!(["blink.ino:12", "setup"])
    );
    // With a condition, as GDB's break writes it.
    let r = c
        .call(
            "breakpoints.set",
            json!({ "project": "blink", "breakpoints": ["blink.ino:12 if i == 10", "setup"] }),
        )
        .await
        .unwrap();
    assert_eq!(
        r["breakpoints"],
        json!(["blink.ino:12 if i == 10", "setup"])
    );
    // Only places debug.break takes; nothing is kept from a refused list.
    for bad in [
        json!({ "project": "blink", "breakpoints": ["-exec"] }),
        json!({ "project": "blink", "breakpoints": ["setup if $_shell(\"x\")"] }),
        json!({ "project": "blink", "breakpoints": ["setup if "] }),
        json!({ "project": "" , "breakpoints": [] }),
    ] {
        match c.call("breakpoints.set", bad).await {
            Err(ClientError::Rpc(e)) => assert_eq!(e.code, code::INVALID_PARAMS),
            other => panic!("expected invalid params, got {other:?}"),
        }
    }
    let r = c
        .call(
            "breakpoints.set",
            json!({ "project": "blink", "breakpoints": [] }),
        )
        .await
        .unwrap();
    assert_eq!(r["breakpoints"], json!([]));
    assert_eq!(
        c.call("breakpoints.get", get).await.unwrap()["breakpoints"],
        json!([])
    );
}

#[tokio::test]
async fn library_export_saves_a_sketch_for_the_arduino_ide() {
    let (_d, sock, _lock) = daemon().await;
    let mut c = Client::connect(&sock).await.unwrap();
    with_library(&mut c).await;
    let out = tempfile::tempdir().unwrap();
    let def = method_def("library.export");
    let params = json!({ "id": "blink", "board": "esp32", "dir": out.path() });
    assert_valid(&def["params"], &params, "library.export params");
    let r = c.call("library.export", params.clone()).await.unwrap();
    assert_valid(&def["result"], &r, "library.export result");
    let ino = out.path().join("blink/blink.ino");
    assert_eq!(Path::new(r["path"].as_str().unwrap()), ino);
    assert!(!std::fs::read_to_string(&ino).unwrap().is_empty());
    assert_eq!(r["libraries"], json!(["Adafruit NeoPixel"]));
    // With reuse, the saved sketch is kept as it is.
    std::fs::write(&ino, "// mine").unwrap();
    let again = c
        .call(
            "library.export",
            json!({ "id": "blink", "board": "esp32", "dir": out.path(), "reuse": true }),
        )
        .await
        .unwrap();
    assert_valid(&def["result"], &again, "library.export result");
    assert_eq!(again["existing"], true);
    assert_eq!(std::fs::read_to_string(&ino).unwrap(), "// mine");
    // Not again over it, not prebuilt firmware, not a relative folder.
    for (bad, want) in [
        (params, code::FAILED),
        (
            json!({ "id": "probeharbor-probe", "board": "esp32", "dir": out.path() }),
            code::FAILED,
        ),
        (
            json!({ "id": "blink", "board": "esp32", "dir": "rel" }),
            code::INVALID_PARAMS,
        ),
    ] {
        match c.call("library.export", bad.clone()).await {
            Err(ClientError::Rpc(e)) => assert_eq!(e.code, want, "{bad}: {}", e.message),
            other => panic!("{bad}: expected an error, got {other:?}"),
        }
    }
}

#[cfg(unix)]
#[tokio::test]
async fn a_failed_build_names_its_first_error() {
    let (d, sock, _lock) = daemon().await;
    std::fs::write(d.path().join("fake-arduino-cli.fail"), "").unwrap();
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
    // The error lines are marked as errors on the way.
    let mut errors = 0;
    let done = loop {
        let n = tokio::time::timeout(std::time::Duration::from_secs(10), c.next_notification())
            .await
            .unwrap()
            .unwrap();
        let event = n["method"].as_str().unwrap().to_owned();
        assert_valid(&method_def(&event)["params"], &n["params"], &event);
        assert_eq!(n["params"]["subscription"], r["job"]);
        if event == "build.finished" {
            break n["params"].clone();
        }
        if n["params"]["level"] == "error" {
            errors += 1;
            // And say where they point, for the app to show the line.
            assert_eq!(
                n["params"]["location"],
                json!({ "file": "blink.ino", "line": 12, "column": 3 })
            );
        } else {
            assert!(n["params"].get("location").is_none(), "{n}");
        }
    };
    assert_eq!(errors, 1);
    assert_eq!(done["ok"], false, "{done}");
    assert_eq!(
        done["firstError"],
        json!({ "file": "blink.ino", "line": 12, "column": 3, "message": "'ledd' was not declared in this scope" })
    );
    // Every diagnostic (protocol 0.44), checked against build.finished's schema.
    assert_valid(
        &method_def("build.finished")["params"],
        &done,
        "build.finished params",
    );
    let all = done["diagnostics"]
        .as_array()
        .unwrap_or_else(|| panic!("no diagnostics: {done}"));
    assert_eq!(all[0]["severity"], "error", "{done}");
    assert_eq!(all[0]["file"], "blink.ino", "{done}");
    assert_eq!(all[0]["line"], 12, "{done}");
}

#[tokio::test]
async fn build_refuses_prebuilt_and_unknown_projects() {
    let (_d, sock, _lock) = daemon().await;
    let mut c = Client::connect(&sock).await.unwrap();
    with_library(&mut c).await;
    for (project, board, want) in [
        // Prebuilt firmware the library links an image of is downloaded, after asking.
        ("probeharbor-probe", "esp32", code::CONSENT_REQUIRED),
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

#[tokio::test]
async fn console_save_writes_the_text_where_asked() {
    let (d, sock, _lock) = daemon().await;
    let mut c = Client::connect(&sock).await.unwrap();
    let path = d.path().join("console.txt");
    let params = json!({ "path": path, "text": "ets Jul 29 2019\ncafé\n" });
    assert_valid(
        &method_def("console.save")["params"],
        &params,
        "console.save params",
    );
    let r = c.call("console.save", params).await.unwrap();
    assert_valid(
        &method_def("console.save")["result"],
        &r,
        "console.save result",
    );
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "ets Jul 29 2019\ncafé\n"
    );
    assert_eq!(r["bytes"], "ets Jul 29 2019\ncafé\n".len());
    match c
        .call(
            "console.save",
            json!({ "path": "relative.txt", "text": "x" }),
        )
        .await
    {
        Err(ClientError::Rpc(e)) => assert_eq!(e.code, code::INVALID_PARAMS),
        other => panic!("wrote to a relative path: {other:?}"),
    }
    match c
        .call(
            "console.save",
            json!({ "path": d.path().join("no/such/dir/x.txt"), "text": "x" }),
        )
        .await
    {
        Err(ClientError::Rpc(e)) => assert_eq!(e.code, code::FAILED),
        other => panic!("wrote into a missing folder: {other:?}"),
    }
}

/// The build.finished event of a build job, checking every event on the way.
async fn build_finished(c: &mut Client, job: &Value) -> Value {
    loop {
        let n = tokio::time::timeout(std::time::Duration::from_secs(10), c.next_notification())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&n["params"]["subscription"], job);
        let event = n["method"].as_str().unwrap().to_owned();
        assert_valid(&method_def(&event)["params"], &n["params"], &event);
        if event == "build.finished" {
            return n["params"].clone();
        }
    }
}

/// Prebuilt firmware: consent for exactly the image, then a download that is checked
/// against the library and kept where a build's image goes; a file that does not match
/// is refused and nothing is kept.
#[tokio::test]
async fn prebuilt_firmware_is_downloaded_after_consent_and_checked() {
    use sha2::{Digest, Sha256};
    let lib = tempfile::tempdir().unwrap();
    let firmware = b"\xe9 a merged image stands in here".to_vec();
    std::fs::create_dir_all(lib.path().join("fw")).unwrap();
    std::fs::write(lib.path().join("fw/probe.bin"), &firmware).unwrap();
    let sha: String = Sha256::digest(&firmware)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    // The example manifest, its prebuilt project's image at a path beside it (a local
    // library may do that; a downloaded one must use https).
    let mut m: Value = serde_json::from_slice(&std::fs::read(example_manifest()).unwrap()).unwrap();
    for p in m["projects"].as_array_mut().unwrap() {
        if p["firmware"] == "prebuilt" {
            for b in p["builds"].as_array_mut().unwrap() {
                b["image"] =
                    json!({ "url": "fw/probe.bin", "sha256": sha, "size": firmware.len() });
            }
        }
    }
    let manifest = lib.path().join("desktop-manifest.json");
    std::fs::write(&manifest, serde_json::to_vec(&m).unwrap()).unwrap();
    let (d, sock, _lock) = daemon_with_source(&manifest).await;
    let mut c = Client::connect(&sock).await.unwrap();
    with_library(&mut c).await;

    let params = json!({ "project": "probeharbor-probe", "board": "esp32" });
    let refusal = match c.call("build.start", params.clone()).await {
        Err(ClientError::Rpc(e)) => e,
        other => panic!("expected consent required, got {other:?}"),
    };
    assert_eq!(refusal.code, code::CONSENT_REQUIRED);
    let data = refusal.data.unwrap();
    // What the user is asked about: this image, by hash.
    assert_eq!(data["plan"]["image"]["sha256"], sha);
    assert_eq!(data["plan"]["codeSha256"], sha);
    assert_eq!(data["plan"]["fqbn"], "");
    let mut with_token = params.clone();
    with_token["consent"] = data["token"].clone();
    let r = c.call("build.start", with_token).await.unwrap();
    assert_valid(
        &method_def("build.start")["result"],
        &r,
        "build.start result",
    );
    let done = build_finished(&mut c, &r["job"]).await;
    assert_eq!(done["ok"], true, "{done}");
    let image = PathBuf::from(done["image"].as_str().unwrap());
    assert_eq!(std::fs::read(&image).unwrap(), firmware);
    assert!(image.starts_with(d.path().join("builds")));

    // The file on the server changes: the download is refused, and the image kept
    // before is not replaced by it.
    std::fs::write(lib.path().join("fw/probe.bin"), b"something else entirely").unwrap();
    let token = match c.call("build.start", params.clone()).await {
        Err(ClientError::Rpc(e)) => e.data.unwrap()["token"].clone(),
        other => panic!("expected consent required, got {other:?}"),
    };
    let mut with_token = params;
    with_token["consent"] = token;
    let r = c.call("build.start", with_token).await.unwrap();
    let done = build_finished(&mut c, &r["job"]).await;
    assert_eq!(done["ok"], false);
    assert!(
        done["error"]
            .as_str()
            .unwrap()
            .contains("not the one the library lists"),
        "{done}"
    );
    assert_eq!(std::fs::read(&image).unwrap(), firmware);
}

/// A pty pair: the master fd (the "board") and the slave path (the "port").
#[cfg(unix)]
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
    unsafe {
        assert_eq!(libc::grantpt(master), 0);
        assert_eq!(libc::unlockpt(master), 0);
    }
    let path = pty_slave_name(master);
    (
        unsafe { std::os::fd::OwnedFd::from_raw_fd(master) },
        PathBuf::from(path),
    )
}

#[cfg(target_os = "linux")]
fn pty_slave_name(master: libc::c_int) -> String {
    let mut name = [0 as libc::c_char; 128];
    // SAFETY: ptsname_r writes at most name.len() bytes into the buffer.
    assert_eq!(
        unsafe { libc::ptsname_r(master, name.as_mut_ptr(), name.len()) },
        0
    );
    unsafe { std::ffi::CStr::from_ptr(name.as_ptr()) }
        .to_string_lossy()
        .into_owned()
}

/// macOS has no ptsname_r; ptsname's static buffer is copied straight away.
#[cfg(all(unix, not(target_os = "linux")))]
fn pty_slave_name(master: libc::c_int) -> String {
    // SAFETY: ptsname returns NULL or a NUL-terminated name in a static buffer.
    let p = unsafe { libc::ptsname(master) };
    assert!(!p.is_null(), "ptsname failed");
    unsafe { std::ffi::CStr::from_ptr(p) }
        .to_string_lossy()
        .into_owned()
}

/// A daemon whose serial.open may open `allowed` (a pty) in place of device.list ports.
async fn serial_daemon(
    allowed: &Path,
) -> (tempfile::TempDir, PathBuf, esp32bench_daemon::InstanceLock) {
    serial_daemon_waiting(allowed, std::time::Duration::from_secs(30)).await
}

/// The same, waiting `reconnect_for` for a port opened with reconnect to come back.
async fn serial_daemon_waiting(
    allowed: &Path,
    reconnect_for: std::time::Duration,
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
    st.serial_reconnect_for = reconnect_for;
    st.known_boards = esp32bench_device::Known::new(dir.path().join("boards.json"));
    let router = esp32bench_daemon::router(Arc::new(st));
    tokio::spawn(esp32bench_rpc::serve(listener, Arc::new(router)));
    (dir, sock, lock)
}

fn b64(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

#[cfg(unix)]
fn un_b64(s: &str) -> Vec<u8> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.decode(s).unwrap()
}

/// Read serial.data until `want` bytes came; checks each event's schema.
#[cfg(unix)]
async fn serial_read(c: &mut Client, want: usize) -> Vec<u8> {
    let mut got = Vec::new();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(6);
    while got.len() < want {
        let n = tokio::time::timeout_at(deadline, c.next_notification())
            .await
            .unwrap()
            .unwrap();
        let event = n["method"].as_str().unwrap();
        assert_valid(&method_def(event)["params"], &n["params"], event);
        if event == "serial.data" {
            got.extend(un_b64(n["params"]["data"].as_str().unwrap()));
        }
    }
    got
}

#[cfg(unix)]
#[tokio::test]
#[cfg_attr(
    target_os = "macos",
    ignore = "macOS pseudo-terminals reject the IOSSIOSPEED ioctl serialport sets the baud with"
)]
async fn serial_records_to_a_file() {
    use std::os::fd::AsRawFd;
    let (master, port) = pty();
    let (d, sock, _lock) = serial_daemon(&port).await;
    let mut c = Client::connect(&sock).await.unwrap();
    let sub = c
        .call("serial.open", json!({ "path": port, "baud": 115200 }))
        .await
        .unwrap()["subscription"]
        .as_u64()
        .unwrap();
    let board = |bytes: &[u8]| {
        // SAFETY: write a valid buffer to the pty master.
        unsafe { libc::write(master.as_raw_fd(), bytes.as_ptr().cast(), bytes.len()) };
    };
    let def = method_def("serial.record");
    board(b"before\n");
    serial_read(&mut c, 7).await;

    let log = d.path().join("board.log");
    let params = json!({ "subscription": sub, "path": log });
    assert_valid(&def["params"], &params, "serial.record params");
    let r = c.call("serial.record", params).await.unwrap();
    assert_valid(&def["result"], &r, "serial.record result");
    assert_eq!(r["recording"], true);
    board(b"rst:0x1 \xff\n");
    serial_read(&mut c, 10).await;
    let r = c
        .call("serial.record", json!({ "subscription": sub }))
        .await
        .unwrap();
    assert_valid(&def["result"], &r, "serial.record result");
    assert_eq!(r, json!({ "recording": false, "path": log, "bytes": 10 }));
    // Only what came while recording, as it came.
    assert_eq!(std::fs::read(&log).unwrap(), b"rst:0x1 \xff\n");
    board(b"after\n");
    serial_read(&mut c, 6).await;
    assert_eq!(std::fs::read(&log).unwrap().len(), 10);

    for bad in [
        json!({ "subscription": sub, "path": "relative.log" }),
        json!({ "subscription": sub, "path": d.path() }),
    ] {
        assert!(c.call("serial.record", bad).await.is_err());
    }
    match c
        .call("serial.record", json!({ "subscription": sub + 99 }))
        .await
    {
        Err(ClientError::Rpc(e)) => assert_eq!(e.code, code::NOT_FOUND),
        other => panic!("expected not found, got {other:?}"),
    }
}

#[cfg(unix)]
#[tokio::test]
#[cfg_attr(
    target_os = "macos",
    ignore = "macOS pseudo-terminals reject the IOSSIOSPEED ioctl serialport sets the baud with"
)]
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
    // One deadline for the loop: other events keep arriving, so a per-event
    // timeout would never fire.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(6);
    while got.len() < out.len() {
        let n = tokio::time::timeout_at(deadline, c.next_notification())
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

    // An ESP-IDF log line: shown without its colour codes, with its level and tag.
    let out = b"\x1b[0;31mE (40) app: no sensor\x1b[0m\n";
    // SAFETY: write a valid buffer to the pty master.
    unsafe { libc::write(master.as_raw_fd(), out.as_ptr().cast(), out.len()) };
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(6);
    let mut shown = String::new();
    let lines = loop {
        let n = tokio::time::timeout_at(deadline, c.next_notification())
            .await
            .unwrap()
            .unwrap();
        assert_valid(
            &method_def("serial.data")["params"],
            &n["params"],
            "serial.data",
        );
        shown.push_str(n["params"]["shown"].as_str().unwrap());
        if let Some(l) = n["params"].get("lines") {
            break l.clone();
        }
    };
    shown.retain(|c| c != '\r');
    assert!(shown.ends_with("E (40) app: no sensor\n"), "{shown:?}");
    assert_eq!(lines, json!([{ "level": "error", "tag": "app" }]));

    // A line of numbers comes with its plot row, in pieces or not.
    for piece in [&b"temp:21."[..], b"5 hum:45\n"] {
        // SAFETY: write a valid buffer to the pty master.
        unsafe { libc::write(master.as_raw_fd(), piece.as_ptr().cast(), piece.len()) };
    }
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(6);
    let plot = loop {
        let n = tokio::time::timeout_at(deadline, c.next_notification())
            .await
            .unwrap()
            .unwrap();
        assert_valid(
            &method_def("serial.data")["params"],
            &n["params"],
            "serial.data",
        );
        if let Some(p) = n["params"].get("plot") {
            break p.clone();
        }
    };
    assert_eq!(
        plot,
        json!([[{ "name": "temp", "value": 21.5 }, { "name": "hum", "value": 45.0 }]])
    );
}

/// A crash line from the board comes with the site's explanation of it (the manifest's
/// explain), once.
#[cfg(unix)]
#[tokio::test]
#[cfg_attr(
    target_os = "macos",
    ignore = "macOS pseudo-terminals reject the IOSSIOSPEED ioctl serialport sets the baud with"
)]
async fn serial_explains_a_crash_once() {
    use std::os::fd::AsRawFd;
    let (master, port) = pty();
    let (_d, sock, _lock) = serial_daemon(&port).await;
    let mut c = Client::connect(&sock).await.unwrap();
    with_library(&mut c).await;
    c.call("serial.open", json!({ "path": port, "baud": 115200 }))
        .await
        .unwrap();
    let crash =
        b"Guru Meditation Error: Core  1 panic'ed (LoadProhibited). Exception was unhandled.\n";
    for _ in 0..2 {
        // SAFETY: write a valid buffer to the pty master.
        unsafe { libc::write(master.as_raw_fd(), crash.as_ptr().cast(), crash.len()) };
    }
    let mut found = Vec::new();
    let mut text = String::new();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(6);
    while text.matches("Guru Meditation").count() < 2 {
        let n = tokio::time::timeout_at(deadline, c.next_notification())
            .await
            .unwrap()
            .unwrap();
        assert_valid(
            &method_def("serial.data")["params"],
            &n["params"],
            "serial.data",
        );
        text.push_str(n["params"]["text"].as_str().unwrap());
        if let Some(e) = n["params"]["explain"].as_array() {
            found.extend(e.iter().cloned());
        }
    }
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0]["key"], "panic-load");
    assert_eq!(found[0]["level"], "problem");
    assert!(
        found[0]["url"]
            .as_str()
            .unwrap()
            .starts_with("https://esp32bench.dev/errors/")
    );
}

#[cfg(unix)]
#[tokio::test]
#[cfg_attr(
    target_os = "macos",
    ignore = "macOS pseudo-terminals reject the IOSSIOSPEED ioctl serialport sets the baud with"
)]
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

#[cfg(unix)]
#[tokio::test]
#[cfg_attr(
    target_os = "macos",
    ignore = "macOS pseudo-terminals reject the IOSSIOSPEED ioctl serialport sets the baud with"
)]
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

/// The next event other than serial.data, within `secs`.
#[cfg(unix)]
async fn next_serial_event(c: &mut Client, secs: u64) -> Value {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(secs);
    loop {
        let n = tokio::time::timeout_at(deadline, c.next_notification())
            .await
            .expect("no serial event in time")
            .unwrap();
        if n["method"] != "serial.data" {
            return n;
        }
    }
}

// A real board's output, checked against its project's expectation.
#[cfg(unix)]
#[tokio::test]
#[cfg_attr(
    target_os = "macos",
    ignore = "macOS pseudo-terminals reject the IOSSIOSPEED ioctl serialport sets the baud with"
)]
async fn serial_checks_a_boards_output_against_its_project() {
    use std::os::fd::AsRawFd;
    let (master, port) = pty();
    let (_d, sock, _lock) = serial_daemon(&port).await;
    let mut c = Client::connect(&sock).await.unwrap();
    with_library(&mut c).await;
    let params = json!({ "path": port, "baud": 115200, "expect": "blink" });
    assert_valid(
        &method_def("serial.open")["params"],
        &params,
        "serial.open params",
    );
    c.call("serial.open", params).await.unwrap();
    let first = next_serial_event(&mut c, 10).await;
    assert_eq!(first["method"], "serial.expect", "{first}");
    assert_valid(
        &method_def("serial.expect")["params"],
        &first["params"],
        "serial.expect",
    );
    assert_eq!(first["params"]["missing"], json!(["tick \\d+ ms"]));
    let out = b"boot\ntick 1000 ms\n";
    // SAFETY: write a valid buffer to the pty master.
    unsafe { libc::write(master.as_raw_fd(), out.as_ptr().cast(), out.len()) };
    let done = next_serial_event(&mut c, 10).await;
    assert_eq!(done["method"], "serial.expect", "{done}");
    assert_eq!(done["params"]["missing"], json!([]));
    assert_eq!(done["params"]["timedOut"], false);
}

/// app_main's address in an ESP32 ELF, from nm beside the toolchain's addr2line.
fn app_main_address(addr2line: &std::path::Path, elf: &std::path::Path) -> String {
    let nm = addr2line.with_file_name("xtensa-esp32-elf-nm");
    let syms = std::process::Command::new(&nm).arg(elf).output().unwrap();
    String::from_utf8_lossy(&syms.stdout)
        .lines()
        .find_map(|l| l.strip_suffix(" T app_main").map(str::to_owned))
        .expect("app_main in the ELF")
}

// A board's crash backtrace, decoded from the build it was flashed with.
#[cfg(unix)]
#[tokio::test]
#[cfg_attr(
    target_os = "macos",
    ignore = "macOS pseudo-terminals reject the IOSSIOSPEED ioctl serialport sets the baud with"
)]
async fn serial_decodes_a_boards_backtrace() {
    use std::os::fd::AsRawFd;
    let elf = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../tests/firmware/build/hello-esp32/esp32bench-hello.elf");
    let data = esp32bench_toolchain::Env::default_arduino_data().unwrap_or_default();
    let Some(a2l) =
        esp32bench_build::backtrace::find_addr2line(&data, "esp32").filter(|_| elf.is_file())
    else {
        eprintln!("skipping: needs the hello firmware and the Arduino core's toolchain");
        return;
    };
    let app_main = app_main_address(&a2l, &elf);
    let (master, port) = pty();
    let (d, sock, _lock) = serial_daemon(&port).await;
    let mut c = Client::connect(&sock).await.unwrap();
    with_library(&mut c).await;
    let out = d.path().join("builds/blink/esp32/out");
    std::fs::create_dir_all(&out).unwrap();
    std::fs::copy(&elf, out.join("blink.ino.elf")).unwrap();
    let params =
        json!({ "path": port, "baud": 115200, "decode": { "project": "blink", "board": "esp32" } });
    assert_valid(
        &method_def("serial.open")["params"],
        &params,
        "serial.open params",
    );
    c.call("serial.open", params).await.unwrap();
    let out = format!(
        "Guru Meditation Error\r\nBacktrace: 0x{app_main}:0x3ffb0000 0x00000000:0x3ffb0020\r\n"
    );
    // SAFETY: write a valid buffer to the pty master.
    unsafe { libc::write(master.as_raw_fd(), out.as_ptr().cast(), out.len()) };
    let n = next_serial_event(&mut c, 10).await;
    assert_eq!(n["method"], "serial.backtrace", "{n}");
    assert_valid(
        &method_def("serial.backtrace")["params"],
        &n["params"],
        "serial.backtrace",
    );
    assert_eq!(n["params"]["frames"][0]["func"], "app_main", "{n}");
    assert_eq!(n["params"]["frames"].as_array().unwrap().len(), 1, "{n}");
    // An unknown build's ids are checked like any other.
    match c
        .call("serial.open", json!({ "path": port, "baud": 115200, "decode": { "project": "../x", "board": "esp32" } }))
        .await
    {
        Err(ClientError::Rpc(e)) => assert_eq!(e.code, code::INVALID_PARAMS),
        other => panic!("odd project accepted: {other:?}"),
    }
}

// A backtrace pasted from anywhere, decoded against a build.
#[tokio::test]
async fn decodes_a_pasted_backtrace() {
    let elf = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../tests/firmware/build/hello-esp32/esp32bench-hello.elf");
    let data = esp32bench_toolchain::Env::default_arduino_data().unwrap_or_default();
    let Some(a2l) =
        esp32bench_build::backtrace::find_addr2line(&data, "esp32").filter(|_| elf.is_file())
    else {
        eprintln!("skipping: needs the hello firmware and the Arduino core's toolchain");
        return;
    };
    let app_main = app_main_address(&a2l, &elf);
    let (d, sock, _lock) = daemon().await;
    let mut c = Client::connect(&sock).await.unwrap();
    with_library(&mut c).await;
    let text =
        format!("rst:0xc\nGuru Meditation Error\nBacktrace: 0x{app_main}:0x3ffb0000\nRebooting...");
    let params = json!({ "project": "blink", "board": "esp32", "text": text });
    assert_valid(
        &method_def("backtrace.decode")["params"],
        &params,
        "backtrace.decode params",
    );
    // Not built yet: nothing to decode from.
    match c.call("backtrace.decode", params.clone()).await {
        Err(ClientError::Rpc(e)) => assert_eq!(e.code, code::NOT_FOUND),
        other => panic!("decoded without a build: {other:?}"),
    }
    let out = d.path().join("builds/blink/esp32/out");
    std::fs::create_dir_all(&out).unwrap();
    std::fs::copy(&elf, out.join("blink.ino.elf")).unwrap();
    let r = c.call("backtrace.decode", params).await.unwrap();
    assert_valid(
        &method_def("backtrace.decode")["result"],
        &r,
        "backtrace.decode result",
    );
    assert_eq!(r["backtraces"].as_array().unwrap().len(), 1, "{r}");
    assert_eq!(r["backtraces"][0]["frames"][0]["func"], "app_main", "{r}");
    // A last line without its newline counts too; text with no backtrace gives none.
    let r = c
        .call(
            "backtrace.decode",
            json!({ "project": "blink", "board": "esp32", "text": "hello\nworld" }),
        )
        .await
        .unwrap();
    assert_eq!(r["backtraces"], json!([]));
    for bad in [
        json!({ "project": "../x", "board": "esp32", "text": "" }),
        json!({ "project": "blink", "board": "esp32", "text": "x".repeat(70_000) }),
    ] {
        match c.call("backtrace.decode", bad).await {
            Err(ClientError::Rpc(e)) => assert_eq!(e.code, code::INVALID_PARAMS),
            other => panic!("expected invalid params, got {other:?}"),
        }
    }
}

// A board that resets over and over: said once, with the reason and the crash.
#[cfg(unix)]
#[tokio::test]
#[cfg_attr(
    target_os = "macos",
    ignore = "macOS pseudo-terminals reject the IOSSIOSPEED ioctl serialport sets the baud with"
)]
async fn serial_says_when_a_board_boot_loops() {
    use std::os::fd::AsRawFd;
    let (master, port) = pty();
    let (_d, sock, _lock) = serial_daemon(&port).await;
    let mut c = Client::connect(&sock).await.unwrap();
    c.call("serial.open", json!({ "path": port, "baud": 115200 }))
        .await
        .unwrap();
    let boot = b"Guru Meditation Error: Core  1 panic'ed (LoadProhibited)\r\nRebooting...\r\nrst:0xc (SW_CPU_RESET),boot:0x13 (SPI_FAST_FLASH_BOOT)\r\n";
    for _ in 0..3 {
        // SAFETY: write a valid buffer to the pty master.
        unsafe { libc::write(master.as_raw_fd(), boot.as_ptr().cast(), boot.len()) };
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let n = next_serial_event(&mut c, 10).await;
    assert_eq!(n["method"], "serial.bootLoop", "{n}");
    assert_valid(
        &method_def("serial.bootLoop")["params"],
        &n["params"],
        "serial.bootLoop",
    );
    assert_eq!(n["params"]["resets"], 3);
    assert_eq!(n["params"]["reason"], "SW_CPU_RESET");
    assert_eq!(
        n["params"]["crash"],
        "Guru Meditation Error: Core  1 panic'ed (LoadProhibited)"
    );
}

// A text file sent line by line, with its line ending.
#[cfg(unix)]
#[tokio::test]
#[cfg_attr(
    target_os = "macos",
    ignore = "macOS pseudo-terminals reject the IOSSIOSPEED ioctl serialport sets the baud with"
)]
async fn serial_sends_a_file_line_by_line() {
    use std::os::fd::AsRawFd;
    let (master, port) = pty();
    let (d, sock, _lock) = serial_daemon(&port).await;
    let mut c = Client::connect(&sock).await.unwrap();
    let r = c
        .call("serial.open", json!({ "path": port, "baud": 115200 }))
        .await
        .unwrap();
    let sub = r["subscription"].clone();
    let file = d.path().join("commands.txt");
    std::fs::write(&file, "AT\r\nAT+GMR\n").unwrap();
    let params = json!({ "subscription": sub, "path": file, "ending": "\r\n", "lineDelay": 20 });
    assert_valid(
        &method_def("serial.sendFile")["params"],
        &params,
        "serial.sendFile params",
    );
    let r = c.call("serial.sendFile", params).await.unwrap();
    assert_valid(
        &method_def("serial.sendFile")["result"],
        &r,
        "serial.sendFile result",
    );
    assert_eq!(r, json!({ "lines": 2, "bytes": 12 }));
    // What the "board" received.
    let mut got = Vec::new();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while got.len() < 12 && std::time::Instant::now() < deadline {
        let mut buf = [0u8; 64];
        // SAFETY: read into a valid buffer from the pty master.
        let n = unsafe { libc::read(master.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
        if n > 0 {
            got.extend_from_slice(&buf[..n as usize]);
        }
    }
    assert_eq!(String::from_utf8_lossy(&got), "AT\r\nAT+GMR\r\n");
    for bad in [
        json!({ "subscription": sub, "path": "relative.txt" }),
        json!({ "subscription": sub, "path": file, "ending": "\t" }),
        json!({ "subscription": sub, "path": file, "lineDelay": 20_000 }),
    ] {
        match c.call("serial.sendFile", bad).await {
            Err(ClientError::Rpc(e)) => assert_eq!(e.code, code::INVALID_PARAMS),
            other => panic!("expected invalid params, got {other:?}"),
        }
    }
}

// The library's projects that use a part, from their wiring.
#[tokio::test]
async fn circuit_examples_are_the_projects_that_use_a_part() {
    let (_d, sock, _lock) = daemon().await;
    let mut c = Client::connect(&sock).await.unwrap();
    with_library(&mut c).await;
    let params = json!({ "kind": "led" });
    assert_valid(
        &method_def("circuit.examples")["params"],
        &params,
        "circuit.examples params",
    );
    let r = c.call("circuit.examples", params).await.unwrap();
    assert_valid(
        &method_def("circuit.examples")["result"],
        &r,
        "circuit.examples result",
    );
    let blink = r["projects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == "blink")
        .unwrap_or_else(|| panic!("{r}"))
        .clone();
    assert!(
        blink["boards"]
            .as_array()
            .unwrap()
            .contains(&json!("esp32")),
        "{r}"
    );
    let r = c
        .call("circuit.examples", json!({ "kind": "no-such-part" }))
        .await
        .unwrap();
    assert_eq!(r["projects"], json!([]));
    match c.call("circuit.examples", json!({ "kind": "" })).await {
        Err(ClientError::Rpc(e)) => assert_eq!(e.code, code::INVALID_PARAMS),
        other => panic!("an empty kind accepted: {other:?}"),
    }
}

// The speed whose output reads as text. A pseudo-terminal has no speed, so what it
// carries reads the same at all of them: readable text is found at the first,
// noise at none.
#[cfg(unix)]
#[tokio::test]
#[cfg_attr(
    target_os = "macos",
    ignore = "macOS pseudo-terminals reject the IOSSIOSPEED ioctl serialport sets the baud with"
)]
async fn serial_finds_the_speed_that_reads() {
    use std::os::fd::AsRawFd;
    for (out, expect) in [
        (
            b"rst:0x1 (POWERON_RESET),boot:0x13 (SPI_FAST_FLASH_BOOT) hello there\r\n".to_vec(),
            Some(115200),
        ),
        (
            (0..120u32)
                .map(|i| [0x80u8, 0xfe, 0x13, 0x02][i as usize % 4])
                .collect(),
            None,
        ),
    ] {
        let (master, port) = pty();
        let (_d, sock, _lock) = serial_daemon(&port).await;
        let mut c = Client::connect(&sock).await.unwrap();
        // The "board" talks all the time.
        let fd = master.as_raw_fd();
        let talking = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let still = talking.clone();
        let writer = std::thread::spawn(move || {
            while still.load(std::sync::atomic::Ordering::Relaxed) {
                // SAFETY: write a valid buffer to the pty master, kept open by the test.
                unsafe { libc::write(fd, out.as_ptr().cast(), out.len()) };
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        });
        let params = json!({ "path": port, "reset": false });
        assert_valid(
            &method_def("serial.findBaud")["params"],
            &params,
            "serial.findBaud params",
        );
        let r = c.call("serial.findBaud", params).await.unwrap();
        talking.store(false, std::sync::atomic::Ordering::Relaxed);
        writer.join().unwrap();
        assert_valid(
            &method_def("serial.findBaud")["result"],
            &r,
            "serial.findBaud result",
        );
        assert_eq!(r["baud"].as_u64(), expect, "{r}");
        if expect.is_some() {
            assert_eq!(r["tried"].as_array().unwrap().len(), 1, "{r}");
        } else {
            assert_eq!(r["tried"].as_array().unwrap().len(), 9, "{r}");
        }
    }
}

// A board's pins to be careful with, and its circuit written to a file and read back.
#[tokio::test]
async fn circuits_go_to_files_and_pins_have_notes() {
    let (d, sock, _lock) = daemon().await;
    let mut c = Client::connect(&sock).await.unwrap();
    with_library(&mut c).await;
    let r = c
        .call("circuit.pinNotes", json!({ "board": "esp32" }))
        .await
        .unwrap();
    assert_valid(
        &method_def("circuit.pinNotes")["result"],
        &r,
        "circuit.pinNotes result",
    );
    assert_eq!(r["chip"], "esp32");
    // From the site's data for this board (manifest 1.3): kinds, why, and its layout.
    assert_eq!(r["source"], "board", "{r}");
    let strap = r["notes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["gpio"] == 12)
        .unwrap_or_else(|| panic!("no note for GPIO 12: {r}"));
    assert_eq!(strap["kind"], "strapping");
    assert!(strap["why"].as_str().is_some_and(|w| !w.is_empty()), "{r}");
    assert_eq!(r["layout"]["i2c"], json!({ "sda": 21, "scl": 22 }), "{r}");
    assert!(
        r["layout"]["left"]
            .as_array()
            .is_some_and(|l| !l.is_empty()),
        "{r}"
    );
    assert_eq!(r["layout"]["safe"][0], 4, "{r}");
    // The models, with the site's wiring notes from the part catalogue (manifest 1.3).
    let r = c.call("circuit.models", json!({})).await.unwrap();
    assert_valid(
        &method_def("circuit.models")["result"],
        &r,
        "circuit.models result",
    );
    let led = r["models"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == "led")
        .unwrap_or_else(|| panic!("no LED model: {r}"));
    assert!(
        led["wiringNotes"][0]
            .as_str()
            .is_some_and(|n| n.starts_with("ANODE: ")),
        "{led}"
    );
    match c
        .call("circuit.pinNotes", json!({ "board": "no-such-board" }))
        .await
    {
        Err(ClientError::Rpc(e)) => assert_eq!(e.code, code::NOT_FOUND),
        other => panic!("notes for an unknown board: {other:?}"),
    }

    let circuit = json!({ "schemaVersion": "1.3.0", "parts": [
        { "id": "led-1", "kind": "led", "pins": { "anode": 2 } },
        { "id": "button-1", "kind": "button", "pins": { "pin": 4 } },
    ] });
    c.call(
        "circuit.set",
        json!({ "project": "blink", "board": "esp32", "circuit": circuit }),
    )
    .await
    .unwrap();
    let file = d.path().join("shared.json");
    let r = c
        .call(
            "circuit.export",
            json!({ "project": "blink", "board": "esp32", "path": file }),
        )
        .await
        .unwrap();
    assert_valid(
        &method_def("circuit.export")["result"],
        &r,
        "circuit.export result",
    );
    assert_eq!(r["parts"], 2);
    // Onto another board.
    let r = c
        .call(
            "circuit.import",
            json!({ "project": "blink", "board": "esp32-doit", "path": file }),
        )
        .await
        .unwrap();
    assert_valid(
        &method_def("circuit.import")["result"],
        &r,
        "circuit.import result",
    );
    assert_eq!(r["circuit"]["parts"].as_array().unwrap().len(), 2);
    let got = c
        .call(
            "circuit.get",
            json!({ "project": "blink", "board": "esp32-doit" }),
        )
        .await
        .unwrap();
    assert_eq!(got["circuit"]["parts"], r["circuit"]["parts"]);
    // A file that is not a circuit, or one whose parts share an id, is refused.
    let bad = d.path().join("bad.json");
    for text in [
        "not json",
        r#"{"schemaVersion":"1.3.0","parts":[{"id":"a","kind":"led","pins":{"anode":2}},{"id":"a","kind":"led","pins":{"anode":5}}]}"#,
    ] {
        std::fs::write(&bad, text).unwrap();
        match c
            .call(
                "circuit.import",
                json!({ "project": "blink", "board": "esp32", "path": bad }),
            )
            .await
        {
            Err(ClientError::Rpc(e)) => assert_eq!(e.code, code::INVALID_PARAMS, "{text}"),
            other => panic!("imported {text}: {other:?}"),
        }
    }
}

// The user's notes, and every setting to a file and back on another computer.
#[tokio::test]
async fn notes_and_settings_travel_in_one_file() {
    let (d, sock, _lock) = daemon().await;
    let mut c = Client::connect(&sock).await.unwrap();
    let r = c
        .call("notes.get", json!({ "project": "blink" }))
        .await
        .unwrap();
    assert_valid(&method_def("notes.get")["result"], &r, "notes.get result");
    assert_eq!(r["text"], "");
    let r = c
        .call(
            "notes.set",
            json!({ "project": "blink", "text": "LED moved to GPIO 4." }),
        )
        .await
        .unwrap();
    assert_valid(&method_def("notes.set")["result"], &r, "notes.set result");
    assert_eq!(
        c.call("notes.get", json!({ "project": "blink" }))
            .await
            .unwrap()["text"],
        "LED moved to GPIO 4."
    );

    c.call(
        "breakpoints.set",
        json!({ "project": "blink", "breakpoints": ["setup", "blink.ino:12 if i == 3"] }),
    )
    .await
    .unwrap();
    c.call(
        "watches.set",
        json!({ "project": "blink", "watches": ["count"] }),
    )
    .await
    .unwrap();
    c.call(
        "commands.set",
        json!({ "project": "blink", "commands": ["status"] }),
    )
    .await
    .unwrap();
    c.call("library.star", json!({ "id": "blink", "starred": true }))
        .await
        .unwrap();
    c.call(
        "circuit.set",
        json!({ "project": "blink", "board": "esp32", "circuit": { "schemaVersion": "1.3.0",
            "parts": [{ "id": "led-1", "kind": "led", "pins": { "anode": 2 } }] } }),
    )
    .await
    .unwrap();
    let file = d.path().join("settings.json");
    let r = c
        .call("data.export", json!({ "path": file }))
        .await
        .unwrap();
    assert_valid(
        &method_def("data.export")["result"],
        &r,
        "data.export result",
    );
    assert_eq!(
        r["counts"],
        json!({ "breakpoints": 1, "watches": 1, "commands": 1, "starred": 1, "notes": 1, "circuits": 1 })
    );

    // Another computer: a fresh daemon, with nothing kept yet.
    let (_d2, sock2, _lock2) = daemon().await;
    let mut c2 = Client::connect(&sock2).await.unwrap();
    let r = c2
        .call("data.import", json!({ "path": file }))
        .await
        .unwrap();
    assert_valid(
        &method_def("data.import")["result"],
        &r,
        "data.import result",
    );
    assert_eq!(r["skipped"], 0);
    assert_eq!(
        c2.call("breakpoints.get", json!({ "project": "blink" }))
            .await
            .unwrap()["breakpoints"],
        json!(["setup", "blink.ino:12 if i == 3"])
    );
    assert_eq!(
        c2.call("notes.get", json!({ "project": "blink" }))
            .await
            .unwrap()["text"],
        "LED moved to GPIO 4."
    );
    assert_eq!(
        c2.call(
            "circuit.get",
            json!({ "project": "blink", "board": "esp32" })
        )
        .await
        .unwrap()["circuit"]["parts"][0]["id"],
        "led-1"
    );
    // A bad entry is left out and counted; a file that is not one is refused.
    let mut b: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    b["watches"]["oled"] = json!(["$_shell(\"x\")"]);
    std::fs::write(&file, serde_json::to_vec(&b).unwrap()).unwrap();
    let r = c2
        .call("data.import", json!({ "path": file }))
        .await
        .unwrap();
    assert_eq!(r["skipped"], 1);
    assert_eq!(
        c2.call("watches.get", json!({ "project": "oled" }))
            .await
            .unwrap()["watches"],
        json!([])
    );
    std::fs::write(&file, r#"{"format":"something else"}"#).unwrap();
    match c2.call("data.import", json!({ "path": file })).await {
        Err(ClientError::Rpc(e)) => assert_eq!(e.code, code::INVALID_PARAMS),
        other => panic!("imported something else: {other:?}"),
    }
}

// What is kept on disk, and builds removed: one board's, a project's, all.
#[tokio::test]
async fn cache_says_its_size_and_cleans_builds() {
    let (d, sock, _lock) = daemon().await;
    let mut c = Client::connect(&sock).await.unwrap();
    for (project, board, size) in [
        ("blink", "esp32", 3000),
        ("blink", "esp32c3", 2000),
        ("oled", "esp32", 1000),
    ] {
        let out = d
            .path()
            .join("builds")
            .join(project)
            .join(board)
            .join("out");
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(out.join("x.bin"), vec![0u8; size]).unwrap();
    }
    let r = c.call("cache.usage", json!({})).await.unwrap();
    assert_valid(
        &method_def("cache.usage")["result"],
        &r,
        "cache.usage result",
    );
    assert_eq!(
        r["categories"][0],
        json!({ "kind": "builds", "path": d.path().join("builds"), "bytes": 6000 })
    );
    assert_eq!(r["builds"][0], json!({ "project": "blink", "bytes": 5000 }));
    let clean = |p: Value| {
        assert_valid(
            &method_def("cache.clean")["params"],
            &p,
            "cache.clean params",
        );
        p
    };
    let r = c
        .call(
            "cache.clean",
            clean(json!({ "project": "blink", "board": "esp32" })),
        )
        .await
        .unwrap();
    assert_valid(
        &method_def("cache.clean")["result"],
        &r,
        "cache.clean result",
    );
    assert_eq!(r["freed"], 3000);
    assert!(d.path().join("builds/blink/esp32c3").is_dir());
    assert_eq!(
        c.call("cache.clean", clean(json!({ "project": "blink" })))
            .await
            .unwrap()["freed"],
        2000
    );
    assert_eq!(
        c.call("cache.clean", clean(json!({}))).await.unwrap()["freed"],
        1000
    );
    assert_eq!(
        c.call("cache.clean", clean(json!({}))).await.unwrap()["freed"],
        0
    ); // nothing left
    for bad in [json!({ "board": "esp32" }), json!({ "project": "../x" })] {
        match c.call("cache.clean", bad).await {
            Err(ClientError::Rpc(e)) => assert_eq!(e.code, code::INVALID_PARAMS),
            other => panic!("expected invalid params, got {other:?}"),
        }
    }
}

// Output at the wrong speed: the core says so, once, with a speed to try.
#[cfg(unix)]
#[tokio::test]
#[cfg_attr(
    target_os = "macos",
    ignore = "macOS pseudo-terminals reject the IOSSIOSPEED ioctl serialport sets the baud with"
)]
async fn serial_says_when_the_speed_looks_wrong() {
    use std::os::fd::AsRawFd;
    let (master, port) = pty();
    let (_d, sock, _lock) = serial_daemon(&port).await;
    let mut c = Client::connect(&sock).await.unwrap();
    c.call("serial.open", json!({ "path": port, "baud": 9600 }))
        .await
        .unwrap();
    let junk: Vec<u8> = (0..300u32)
        .map(|i| [0x80u8, 0xfe, 0x13, 0x02][i as usize % 4])
        .collect();
    // SAFETY: write a valid buffer to the pty master.
    unsafe { libc::write(master.as_raw_fd(), junk.as_ptr().cast(), junk.len()) };
    let n = next_serial_event(&mut c, 10).await;
    assert_eq!(n["method"], "serial.garbled", "{n}");
    assert_valid(
        &method_def("serial.garbled")["params"],
        &n["params"],
        "serial.garbled",
    );
    assert_eq!(n["params"]["suggest"], 115200);
}

// The port is a symlink to a pty, so the "board" can come back at the same path.
#[cfg(unix)]
#[tokio::test]
#[cfg_attr(
    target_os = "macos",
    ignore = "macOS pseudo-terminals reject the IOSSIOSPEED ioctl serialport sets the baud with"
)]
async fn serial_reconnects_when_the_board_comes_back() {
    use std::os::fd::AsRawFd;
    let links = tempfile::tempdir().unwrap();
    let port = links.path().join("ttyACM0");
    let (master, first) = pty();
    std::os::unix::fs::symlink(&first, &port).unwrap();
    let (_d, sock, _lock) = serial_daemon(&port).await;
    let mut c = Client::connect(&sock).await.unwrap();
    let params = json!({ "path": port, "baud": 115200, "reconnect": true });
    assert_valid(
        &method_def("serial.open")["params"],
        &params,
        "serial.open params",
    );
    let sub = c.call("serial.open", params).await.unwrap()["subscription"].clone();

    // Reset over USB: the port goes, and comes back as a new device.
    std::fs::remove_file(&port).unwrap();
    drop(master);
    let n = next_serial_event(&mut c, 15).await;
    assert_eq!(n["method"], "serial.lost", "{n}");
    assert_valid(
        &method_def("serial.lost")["params"],
        &n["params"],
        "serial.lost",
    );
    let (master, second) = pty();
    std::os::unix::fs::symlink(&second, &port).unwrap();
    let n = next_serial_event(&mut c, 15).await;
    assert_eq!(n["method"], "serial.reconnected", "{n}");
    assert_valid(
        &method_def("serial.reconnected")["params"],
        &n["params"],
        "serial.reconnected",
    );
    assert_eq!(n["params"]["subscription"], sub);

    // The same subscription reads and writes the new port.
    let out = b"back\n";
    // SAFETY: write a valid buffer to the pty master.
    unsafe { libc::write(master.as_raw_fd(), out.as_ptr().cast(), out.len()) };
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(6);
    let mut got = Vec::new();
    while got.len() < out.len() {
        let n = tokio::time::timeout_at(deadline, c.next_notification())
            .await
            .unwrap()
            .unwrap();
        if n["method"] == "serial.data" {
            got.extend(un_b64(n["params"]["data"].as_str().unwrap()));
        }
    }
    assert_eq!(got, out);
    c.call(
        "serial.write",
        json!({ "subscription": sub, "data": b64(b"hi\n") }),
    )
    .await
    .unwrap();
}

#[cfg(unix)]
#[tokio::test]
#[cfg_attr(
    target_os = "macos",
    ignore = "macOS pseudo-terminals reject the IOSSIOSPEED ioctl serialport sets the baud with"
)]
async fn serial_gives_up_when_the_board_stays_away() {
    // Through a symlink that goes too, so another test's new pty cannot take its place.
    let links = tempfile::tempdir().unwrap();
    let port = links.path().join("ttyACM0");
    let (master, pts) = pty();
    std::os::unix::fs::symlink(&pts, &port).unwrap();
    let (_d, sock, _lock) =
        serial_daemon_waiting(&port, std::time::Duration::from_millis(600)).await;
    let mut c = Client::connect(&sock).await.unwrap();
    c.call(
        "serial.open",
        json!({ "path": port, "baud": 115200, "reconnect": true }),
    )
    .await
    .unwrap();
    std::fs::remove_file(&port).unwrap();
    drop(master);
    assert_eq!(next_serial_event(&mut c, 15).await["method"], "serial.lost");
    assert_eq!(
        next_serial_event(&mut c, 15).await["method"],
        "serial.closed"
    );
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
    // Where the distribution packages arduino-cli (Arch), installing it needs root, so
    // it is refused here; elsewhere the app installs its own copy (not tested here: a
    // download).
    if esp32bench_distro::Distro::detect() == esp32bench_distro::Distro::Arch {
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
    let mut clock = 0;
    // One deadline for the loop: other events keep arriving, so a per-event
    // timeout would never fire.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(90);
    while !text.contains(marker) {
        let n = tokio::time::timeout_at(deadline, c.next_notification())
            .await
            .unwrap_or_else(|_| panic!("no `{marker}` in: {text}"))
            .unwrap();
        let event = n["method"].as_str().unwrap();
        assert_valid(&method_def(event)["params"], &n["params"], event);
        assert_eq!(&n["params"]["subscription"], sub);
        // The project's expectation is checked alongside (sim.expect has its own test),
        // and the simulated time comes twice a second; it only goes forward.
        if event == "sim.expect" {
            continue;
        }
        if event == "sim.clock" {
            let ns = n["params"]["ns"].as_u64().unwrap();
            assert!(ns >= clock, "the clock went back: {clock} then {ns}");
            clock = ns;
            continue;
        }
        assert_eq!(event, "sim.output", "{n}");
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

/// The next `event` of `sub`, every event checked against its schema on the way; the
/// output text seen before it is returned too.
async fn sim_event(c: &mut Client, sub: &Value, event: &str) -> (Value, String) {
    let mut text = String::new();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(90);
    loop {
        let n = tokio::time::timeout_at(deadline, c.next_notification())
            .await
            .unwrap_or_else(|_| panic!("no {event}; output: {text}"))
            .unwrap();
        let method = n["method"].as_str().unwrap();
        assert_valid(&method_def(method)["params"], &n["params"], method);
        assert_eq!(&n["params"]["subscription"], sub);
        if method == "sim.output" {
            text.push_str(n["params"]["text"].as_str().unwrap());
        }
        if method == event {
            return (n["params"].clone(), text);
        }
        assert_ne!(method, "sim.exited", "{n}");
    }
}

/// sim.pause stops the firmware (its input waits) and resumes it; a backtrace in the
/// output is decoded from the build's ELF (sim.backtrace). The hello firmware echoes
/// what it is sent, so echoing a backtrace line naming app_main's address stands in
/// for a crash.
#[tokio::test]
async fn sim_pauses_and_decodes_backtraces() {
    let elf = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../tests/firmware/build/hello-esp32/esp32bench-hello.elf");
    let Some(hello) = firmware("hello-esp32.bin").filter(|_| elf.is_file()) else {
        eprintln!("skipping: hello firmware not built");
        return;
    };
    let data = esp32bench_toolchain::Env::default_arduino_data().unwrap_or_default();
    let Some(a2l) = esp32bench_build::backtrace::find_addr2line(&data, "esp32") else {
        eprintln!("skipping: needs the Arduino core's toolchain");
        return;
    };
    if esp32bench_sim::install::find_qemu(esp32bench_sim::Arch::Xtensa).is_err() {
        eprintln!("skipping: QEMU not installed");
        return;
    }
    let app_main = app_main_address(&a2l, &elf);

    let (d, sock, _lock) = daemon().await;
    let mut c = Client::connect(&sock).await.unwrap();
    with_library(&mut c).await;
    let out = d.path().join("builds/blink/esp32/out");
    std::fs::create_dir_all(&out).unwrap();
    std::fs::copy(&hello, out.join("blink.ino.merged.bin")).unwrap();
    std::fs::copy(&elf, out.join("blink.ino.elf")).unwrap();
    let r = c
        .call("sim.start", json!({ "project": "blink", "board": "esp32" }))
        .await
        .unwrap();
    let sub = r["subscription"].clone();
    sim_wait_for(&mut c, &sub, "esp32bench-hello ready").await;

    use base64::Engine;
    let send = |s: &str| base64::engine::general_purpose::STANDARD.encode(s.as_bytes());
    let r = c
        .call("sim.pause", json!({ "subscription": sub, "paused": true }))
        .await
        .unwrap();
    assert_valid(&method_def("sim.pause")["result"], &r, "sim.pause result");
    assert_eq!(r["paused"], true);
    let (e, _) = sim_event(&mut c, &sub, "sim.paused").await;
    assert_eq!(e["paused"], true);
    c.call(
        "sim.write",
        json!({ "subscription": sub, "data": send("ping 7\r\n") }),
    )
    .await
    .unwrap();
    // Paused: no echo for a while.
    let mut text = String::new();
    let quiet = tokio::time::Instant::now() + std::time::Duration::from_millis(1500);
    while let Ok(n) = tokio::time::timeout_at(quiet, c.next_notification()).await {
        let n = n.unwrap();
        if n["method"] == "sim.output" {
            text.push_str(n["params"]["text"].as_str().unwrap());
        }
    }
    assert!(!text.contains("echo ping 7"), "echoed while paused: {text}");
    c.call("sim.pause", json!({ "subscription": sub, "paused": false }))
        .await
        .unwrap();
    let (e, _) = sim_event(&mut c, &sub, "sim.paused").await;
    assert_eq!(e["paused"], false);
    sim_wait_for(&mut c, &sub, "esp32bench-hello echo ping 7").await;

    // Run for 300 ms of simulated time, then pause there (on lcgamboa's QEMU, which
    // reads the simulated clock).
    let params = json!({ "subscription": sub, "ms": 300 });
    assert_valid(
        &method_def("sim.runFor")["params"],
        &params,
        "sim.runFor params",
    );
    if expected_engine() == "lcgamboa" {
        let (clock, _) = sim_event(&mut c, &sub, "sim.clock").await;
        c.call("sim.runFor", params).await.unwrap();
        let paused = loop {
            let (e, _) = sim_event(&mut c, &sub, "sim.paused").await;
            if e["paused"] == true {
                break e;
            }
        };
        let ns = paused["ns"].as_u64().unwrap();
        assert!(
            ns >= clock["ns"].as_u64().unwrap() + 300_000_000,
            "{paused} after {clock}"
        );
        // From a pause, it goes on for the time asked, then pauses again.
        c.call("sim.runFor", json!({ "subscription": sub, "ms": 100 }))
            .await
            .unwrap();
        let (going, _) = sim_event(&mut c, &sub, "sim.paused").await;
        assert_eq!(going["paused"], false, "{going}");
        let (again, _) = sim_event(&mut c, &sub, "sim.paused").await;
        assert_eq!(again["paused"], true, "{again}");
        let later = again["ns"].as_u64().unwrap();
        assert!(
            (ns + 100_000_000..ns + 400_000_000).contains(&later),
            "{again} after {paused}"
        );
        c.call("sim.pause", json!({ "subscription": sub, "paused": false }))
            .await
            .unwrap();
        sim_event(&mut c, &sub, "sim.paused").await;
    } else {
        match c.call("sim.runFor", params).await {
            Err(ClientError::Rpc(e)) => assert_eq!(e.code, code::FAILED),
            other => panic!("runFor without the simulated clock: {other:?}"),
        }
    }

    // A file of lines sent to the console: each echoed.
    let file = d.path().join("lines.txt");
    std::fs::write(&file, "ping 8\nping 9\n").unwrap();
    let r = c
        .call(
            "sim.sendFile",
            json!({ "subscription": sub, "path": file, "ending": "\r\n" }),
        )
        .await
        .unwrap();
    assert_eq!(r["lines"], 2, "{r}");
    sim_wait_for(&mut c, &sub, "esp32bench-hello echo ping 9").await;

    let line = format!("Backtrace: 0x{app_main}:0x3ffb0000\r\n");
    c.call(
        "sim.write",
        json!({ "subscription": sub, "data": send(&line) }),
    )
    .await
    .unwrap();
    let (bt, _) = sim_event(&mut c, &sub, "sim.backtrace").await;
    assert_eq!(bt["frames"][0]["func"], "app_main", "{bt}");
    assert!(
        bt["frames"][0]["file"]
            .as_str()
            .unwrap()
            .ends_with("hello.c"),
        "{bt}"
    );
    // Not the sketch: the project is blink, the file hello.c.
    assert!(bt["frames"][0].get("sketch").is_none(), "{bt}");

    // Three resets in a row (echoed reset lines stand in for them): a boot loop.
    for _ in 0..3 {
        c.call(
            "sim.write",
            json!({ "subscription": sub, "data": send("rst:0xc (SW_CPU_RESET),boot:0x13\r\n") }),
        )
        .await
        .unwrap();
    }
    let (b, _) = sim_event(&mut c, &sub, "sim.bootLoop").await;
    assert_eq!(b["resets"], 3, "{b}");
    assert_eq!(b["reason"], "SW_CPU_RESET", "{b}");
    c.call("core.unsubscribe", json!({ "subscription": sub }))
        .await
        .unwrap();
}

/// The project's expectation (the manifest's expect) is checked as the output comes:
/// sim.expect says what is missing at the start and when it has all appeared.
#[tokio::test]
async fn sim_checks_the_output_against_the_projects_expectation() {
    let Some(hello) = firmware("hello-esp32.bin") else {
        eprintln!("skipping: hello firmware not built");
        return;
    };
    if esp32bench_sim::install::find_qemu(esp32bench_sim::Arch::Xtensa).is_err() {
        eprintln!("skipping: QEMU not installed");
        return;
    }
    // The example library, with blink expecting what the hello firmware prints.
    let lib = tempfile::tempdir().unwrap();
    let mut m: Value = serde_json::from_slice(&std::fs::read(example_manifest()).unwrap()).unwrap();
    for p in m["projects"].as_array_mut().unwrap() {
        if p["id"] == "blink" {
            p["expect"] = json!({ "patterns": [r"esp32bench-hello ready", r"never \d+ printed"], "seconds": 1, "needs": "none" });
        }
    }
    let manifest = lib.path().join("m.json");
    std::fs::write(&manifest, serde_json::to_vec(&m).unwrap()).unwrap();
    let (d, sock, _lock) = daemon_with_source(&manifest).await;
    let mut c = Client::connect(&sock).await.unwrap();
    with_library(&mut c).await;
    let out = d.path().join("builds/blink/esp32/out");
    std::fs::create_dir_all(&out).unwrap();
    std::fs::copy(&hello, out.join("blink.ino.merged.bin")).unwrap();
    let r = c
        .call("sim.start", json!({ "project": "blink", "board": "esp32" }))
        .await
        .unwrap();
    let sub = r["subscription"].clone();
    // Collect sim.expect until it times out (one pattern never appears).
    let mut seen: Vec<Value> = Vec::new();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(90);
    while !seen.last().is_some_and(|e| e["timedOut"] == true) {
        let n = tokio::time::timeout_at(deadline, c.next_notification())
            .await
            .unwrap_or_else(|_| panic!("no timeout in {seen:?}"))
            .unwrap();
        let event = n["method"].as_str().unwrap();
        assert_valid(&method_def(event)["params"], &n["params"], event);
        if event == "sim.expect" {
            seen.push(n["params"].clone());
        }
    }
    // At the start: nothing seen.
    assert_eq!(seen[0]["matched"], json!([]));
    assert_eq!(seen[0]["needs"], "none");
    // Then the ready line, and at the end the one that never came.
    assert!(
        seen.iter()
            .any(|e| e["matched"] == json!(["esp32bench-hello ready"])),
        "{seen:?}"
    );
    let last = seen.last().unwrap();
    assert_eq!(last["missing"], json!([r"never \d+ printed"]));
    c.call("core.unsubscribe", json!({ "subscription": sub }))
        .await
        .unwrap();
}

/// sim.start with icount: true runs QEMU counting time in instructions; the build
/// still boots and talks, and the result says so.
#[tokio::test]
async fn sim_counts_time_in_instructions_when_asked() {
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
        .call(
            "sim.start",
            json!({ "project": "blink", "board": "esp32", "icount": true, "network": false }),
        )
        .await
        .unwrap();
    assert_valid(&method_def("sim.start")["result"], &r, "sim.start result");
    assert_eq!(r["icount"], true);
    let sub = r["subscription"].clone();
    sim_wait_for(&mut c, &sub, "esp32bench-hello ready").await;
    let qemu = std::fs::read_dir("/proc")
        .unwrap()
        .filter_map(|e| std::fs::read_to_string(e.ok()?.path().join("cmdline")).ok())
        .any(|c| c.contains("esp32bench") && c.contains("-icount\0shift=2,sleep=on"));
    assert!(qemu, "no QEMU running with -icount");
}

/// On lcgamboa's fork, sim.start gives the firmware the simulated Wi-Fi network and
/// joins a library sketch to it (its settings block reports Wi-Fi unset); the
/// sketch's web page answers on the result's webUrl. With network: false there is
/// none.
#[tokio::test]
async fn sim_joins_a_sketch_to_the_simulated_wifi() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let Some(wifi) = firmware("arduino-wifi-esp32.bin") else {
        eprintln!("skipping: arduino-wifi firmware not built");
        return;
    };
    if expected_engine() != "lcgamboa" {
        eprintln!("skipping: the simulated network needs lcgamboa's QEMU");
        return;
    }
    let (d, sock, _lock) = daemon().await;
    let mut c = Client::connect(&sock).await.unwrap();
    with_library(&mut c).await;
    let out = d.path().join("builds/blink/esp32/out");
    std::fs::create_dir_all(&out).unwrap();
    std::fs::copy(&wifi, out.join("blink.ino.merged.bin")).unwrap();

    let r = c
        .call(
            "sim.start",
            json!({ "project": "blink", "board": "esp32", "network": false }),
        )
        .await
        .unwrap();
    assert_valid(&method_def("sim.start")["result"], &r, "sim.start result");
    assert!(r["network"].is_null(), "{r}");
    c.call(
        "core.unsubscribe",
        json!({ "subscription": r["subscription"] }),
    )
    .await
    .unwrap();

    let r = c
        .call("sim.start", json!({ "project": "blink", "board": "esp32" }))
        .await
        .unwrap();
    assert_valid(&method_def("sim.start")["result"], &r, "sim.start result");
    assert_eq!(r["network"]["ssid"], "PICSimLabWifi");
    let url = r["network"]["webUrl"].as_str().unwrap().to_owned();
    let port: u16 = url
        .trim_start_matches("http://127.0.0.1:")
        .trim_end_matches('/')
        .parse()
        .unwrap();
    let sub = r["subscription"].clone();
    let (mut joined, mut text) = (false, String::new());
    // One deadline for the whole start-up: pin and output events keep coming, so a
    // per-event timeout never fires if the sketch never serves.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(90);
    while !text.contains("esp32bench-wifi serving") {
        let n = tokio::time::timeout_at(deadline, c.next_notification())
            .await
            .unwrap_or_else(|_| panic!("not serving after 90 s: {text}"))
            .unwrap();
        let event = n["method"].as_str().unwrap();
        assert_valid(&method_def(event)["params"], &n["params"], event);
        // The first simulation's last events (its sim.expect goes out at once) can
        // still arrive after its unsubscribe.
        if n["params"]["subscription"] != sub {
            continue;
        }
        match event {
            "sim.wifiJoined" => {
                assert_eq!(n["params"]["ssid"], "PICSimLabWifi");
                joined = true;
            }
            "sim.output" => text.push_str(n["params"]["text"].as_str().unwrap()),
            _ => {}
        }
    }
    assert!(joined, "no sim.wifiJoined: {text}");
    assert!(text.contains("esp32bench-wifi ip=10.0.2.15"), "{text}");
    assert!(text.contains("esp32bench-wifi out=0"), "{text}");
    let mut tcp = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    tcp.write_all(b"GET / HTTP/1.0\r\n\r\n").await.unwrap();
    let mut reply = String::new();
    tokio::time::timeout(
        std::time::Duration::from_secs(30),
        tcp.read_to_string(&mut reply),
    )
    .await
    .expect("no web reply in 30 s")
    .unwrap();
    assert!(reply.contains("esp32bench-web ok"), "{reply}");
}

/// A local ESP-IDF project: local.open refuses a folder that is not one and remembers
/// one that is; build.start builds it with idf.py without asking for consent (the user
/// chose it); sim.start runs the build. tests/firmware/hello is an ESP-IDF project.
/// The build and the simulation need ESP-IDF and QEMU, and skip without them.
#[tokio::test]
async fn local_idf_projects_open_build_and_run() {
    let (d, sock, _lock) = daemon().await;
    let mut c = Client::connect(&sock).await.unwrap();
    with_library(&mut c).await;

    let not = d.path().join("not-idf");
    std::fs::create_dir_all(&not).unwrap();
    match c.call("local.open", json!({ "path": not })).await {
        Err(ClientError::Rpc(e)) => assert_eq!(e.code, code::INVALID_PARAMS, "{}", e.message),
        other => panic!("opened a folder that is no project: {other:?}"),
    }

    // A copy, so the test cannot touch the repo's project.
    let hello = d.path().join("hello");
    let src =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../tests/firmware/hello");
    for f in [
        "CMakeLists.txt",
        "sdkconfig.defaults",
        "main/CMakeLists.txt",
        "main/hello.c",
    ] {
        if let Some(parent) = hello.join(f).parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::copy(src.join(f), hello.join(f)).unwrap();
    }
    let r = c
        .call("local.open", json!({ "path": hello }))
        .await
        .unwrap();
    assert_valid(&method_def("local.open")["result"], &r, "local.open result");
    let id = r["project"]["id"].as_str().unwrap().to_owned();
    assert_eq!(r["project"]["name"], "esp32bench-hello");
    // Opening it again gives the same project.
    let again = c
        .call("local.open", json!({ "path": hello }))
        .await
        .unwrap();
    assert_eq!(again["project"]["id"], id.as_str());

    let list = c.call("local.list", json!({})).await.unwrap();
    assert_valid(
        &method_def("local.list")["result"],
        &list,
        "local.list result",
    );
    assert_eq!(list["projects"].as_array().unwrap().len(), 1);
    let got = c.call("local.get", json!({ "id": id })).await.unwrap();
    assert_valid(&method_def("local.get")["result"], &got, "local.get result");
    let boards: Vec<&str> = got["boards"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["id"].as_str().unwrap())
        .collect();
    assert_eq!(boards, ["esp32", "esp32s3", "esp32c3"]);
    assert_eq!(got["exists"], true);
    // Its source files, to open in the user's editor.
    assert_eq!(
        got["sources"],
        json!(["CMakeLists.txt", "main/CMakeLists.txt", "main/hello.c"])
    );

    if got["idf"].is_null() {
        eprintln!("skipping the build: ESP-IDF is not installed");
    } else {
        let r = c
            .call("build.start", json!({ "project": id, "board": "esp32c3" }))
            .await
            .unwrap();
        assert_valid(
            &method_def("build.start")["result"],
            &r,
            "build.start result",
        );
        assert_eq!(r["plan"]["target"], "esp32c3");
        let job = r["job"].clone();
        let finished = loop {
            let n =
                tokio::time::timeout(std::time::Duration::from_secs(900), c.next_notification())
                    .await
                    .expect("the ESP-IDF build did not finish")
                    .unwrap();
            assert_eq!(n["params"]["subscription"], job);
            if n["method"] == "build.finished" {
                break n;
            }
        };
        assert_eq!(finished["params"]["ok"], true, "{finished}");

        if esp32bench_sim::install::find_qemu(esp32bench_sim::Arch::Riscv32).is_ok()
            || expected_engine() == "lcgamboa"
        {
            let r = c
                .call(
                    "sim.start",
                    json!({ "project": id, "board": "esp32c3", "network": false }),
                )
                .await
                .unwrap();
            sim_wait_for(&mut c, &r["subscription"], "esp32bench-hello ready").await;
            c.call(
                "core.unsubscribe",
                json!({ "subscription": r["subscription"] }),
            )
            .await
            .unwrap();
        }
    }

    let r = c.call("local.remove", json!({ "id": id })).await.unwrap();
    assert_eq!(r["removed"], true);
    assert!(
        hello.join("CMakeLists.txt").is_file(),
        "the folder was touched"
    );
    match c.call("local.get", json!({ "id": id })).await {
        Err(ClientError::Rpc(e)) => assert_eq!(e.code, code::NOT_FOUND),
        other => panic!("a forgotten project is still there: {other:?}"),
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
        .call(
            "debug.evaluate",
            json!({ "subscription": sub, "expression": "6 * 7" }),
        )
        .await
        .unwrap();
    assert_valid(
        &method_def("debug.evaluate")["result"],
        &r,
        "debug.evaluate result",
    );
    assert_eq!(r["value"], "42");
    match c
        .call(
            "debug.evaluate",
            json!({ "subscription": sub, "expression": "$_shell(\"true\")" }),
        )
        .await
    {
        Err(ClientError::Rpc(e)) => assert_eq!(e.code, code::INVALID_PARAMS),
        other => panic!("$_shell accepted: {other:?}"),
    }
    // The pause the debugger owns is not sim.pause's.
    match c
        .call("sim.pause", json!({ "subscription": sub, "paused": true }))
        .await
    {
        Err(ClientError::Rpc(e)) => assert_eq!(e.code, code::FAILED),
        other => panic!("sim.pause on a debug run: {other:?}"),
    }

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

    // Step over a line. On a loaded machine GDB has once reported the breakpoint the
    // step started from as hit again; then step again. Any other stop fails.
    let mut stop = Value::Null;
    for _ in 0..2 {
        c.call(
            "debug.control",
            json!({ "subscription": sub, "action": "next" }),
        )
        .await
        .unwrap();
        stop = debug_wait(&mut c, &sub, "debug.stopped").await;
        if !(stop["reason"] == "breakpoint-hit" && stop["breakpoint"] == number) {
            break;
        }
        eprintln!("the step reported breakpoint {number} again: {stop}");
    }
    assert_eq!(stop["reason"], "end-stepping-range", "{stop}");

    // Run to a line: the tick's printf, reached within a second.
    match c
        .call(
            "debug.control",
            json!({ "subscription": sub, "action": "runTo" }),
        )
        .await
    {
        Err(ClientError::Rpc(e)) => assert_eq!(e.code, code::INVALID_PARAMS),
        other => panic!("runTo without a location: {other:?}"),
    }
    c.call(
        "debug.control",
        json!({ "subscription": sub, "action": "runTo", "location": "hello.c:37" }),
    )
    .await
    .unwrap();
    let stop = debug_wait(&mut c, &sub, "debug.stopped").await;
    assert_eq!(stop["frame"]["line"], 37, "{stop}");

    // A breakpoint with a condition stops only when it holds: the second tick.
    let r = c
        .call(
            "debug.break",
            json!({ "subscription": sub, "location": "hello.c:37", "condition": "tick == 2" }),
        )
        .await
        .unwrap();
    assert_valid(
        &method_def("debug.break")["result"],
        &r,
        "debug.break result",
    );
    assert_eq!(r["breakpoint"]["condition"], "tick == 2", "{r}");
    let conditional = r["breakpoint"]["number"].clone();
    c.call(
        "debug.control",
        json!({ "subscription": sub, "action": "continue" }),
    )
    .await
    .unwrap();
    let stop = debug_wait(&mut c, &sub, "debug.stopped").await;
    assert_eq!(stop["breakpoint"], conditional, "{stop}");
    let r = c
        .call(
            "debug.evaluate",
            json!({ "subscription": sub, "expression": "tick" }),
        )
        .await
        .unwrap();
    assert_eq!(r["value"], "2");
    c.call(
        "debug.delete",
        json!({ "subscription": sub, "breakpoint": conditional }),
    )
    .await
    .unwrap();

    // Memory: the chip info's first four bytes, its model, little-endian 1 (CHIP_ESP32).
    let r = c
        .call(
            "debug.memory",
            json!({ "subscription": sub, "address": "&info", "length": 4 }),
        )
        .await
        .unwrap();
    assert_valid(
        &method_def("debug.memory")["result"],
        &r,
        "debug.memory result",
    );
    assert_eq!(r["bytes"], "01000000", "{r}");
    assert!(r["address"].as_str().unwrap().starts_with("0x"), "{r}");
    match c
        .call(
            "debug.memory",
            json!({ "subscription": sub, "address": "&info", "length": 5000 }),
        )
        .await
    {
        Err(ClientError::Rpc(e)) => assert_eq!(e.code, code::INVALID_PARAMS),
        other => panic!("too long a read accepted: {other:?}"),
    }

    // A structure's members, each with an expression to look inside it.
    let r = c
        .call(
            "debug.children",
            json!({ "subscription": sub, "expression": "info" }),
        )
        .await
        .unwrap();
    assert_valid(
        &method_def("debug.children")["result"],
        &r,
        "debug.children result",
    );
    let names: Vec<&str> = r["children"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"model") && names.contains(&"cores"), "{r}");
    let cores = r["children"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["name"] == "cores")
        .unwrap()
        .clone();
    assert_eq!(cores["expression"], "(info).cores", "{r}");
    // And a value set, as the firmware would see it.
    let r = c
        .call(
            "debug.assign",
            json!({ "subscription": sub, "expression": "info.cores", "value": "3" }),
        )
        .await
        .unwrap();
    assert_valid(
        &method_def("debug.assign")["result"],
        &r,
        "debug.assign result",
    );
    assert!(r["value"].as_str().unwrap().starts_with('3'), "{r}");
    let r = c
        .call(
            "debug.evaluate",
            json!({ "subscription": sub, "expression": "info.cores * 2" }),
        )
        .await
        .unwrap();
    assert_eq!(r["value"], "6");
    match c
        .call(
            "debug.assign",
            json!({ "subscription": sub, "expression": "info.cores", "value": "1; x" }),
        )
        .await
    {
        Err(ClientError::Rpc(e)) => assert_eq!(e.code, code::INVALID_PARAMS),
        other => panic!("a value with ; accepted: {other:?}"),
    }

    // A watchpoint stops when FreeRTOS's tick count changes, with the old and new value.
    let r = c
        .call(
            "debug.watch",
            json!({ "subscription": sub, "expression": "xTickCount" }),
        )
        .await
        .unwrap();
    assert_valid(
        &method_def("debug.watch")["result"],
        &r,
        "debug.watch result",
    );
    let wpt = r["watchpoint"]["number"].clone();
    c.call(
        "debug.control",
        json!({ "subscription": sub, "action": "continue" }),
    )
    .await
    .unwrap();
    let stop = debug_wait(&mut c, &sub, "debug.stopped").await;
    assert_eq!(stop["reason"], "watchpoint-trigger", "{stop}");
    assert_eq!(stop["breakpoint"], wpt, "{stop}");
    assert_eq!(stop["watch"]["expression"], "xTickCount", "{stop}");
    let old: u64 = stop["watch"]["old"].as_str().unwrap().parse().unwrap();
    let new: u64 = stop["watch"]["new"].as_str().unwrap().parse().unwrap();
    assert_eq!(new, old + 1, "{stop}");
    c.call(
        "debug.delete",
        json!({ "subscription": sub, "breakpoint": wpt }),
    )
    .await
    .unwrap();

    // The caller's frame: frame 1's locals, and the stack as before.
    let r = c
        .call("debug.inspect", json!({ "subscription": sub, "frame": 1 }))
        .await
        .unwrap();
    assert_valid(
        &method_def("debug.inspect")["result"],
        &r,
        "debug.inspect result",
    );
    assert_eq!(r["frame"], 1, "{r}");
    assert!(r["frames"].as_array().unwrap().len() >= 2, "{r}");
    let r = c
        .call("debug.inspect", json!({ "subscription": sub }))
        .await
        .unwrap();
    assert_eq!(r["frame"], 0, "{r}");

    // Pass a breakpoint twice, then stop: the third time, and it says so.
    let r = c
        .call(
            "debug.break",
            json!({ "subscription": sub, "location": "hello.c:37", "ignore": 2 }),
        )
        .await
        .unwrap();
    assert_valid(
        &method_def("debug.break")["result"],
        &r,
        "debug.break result",
    );
    assert_eq!(r["breakpoint"]["ignore"], 2, "{r}");
    let counted = r["breakpoint"]["number"].clone();
    c.call(
        "debug.control",
        json!({ "subscription": sub, "action": "continue" }),
    )
    .await
    .unwrap();
    let stop = debug_wait(&mut c, &sub, "debug.stopped").await;
    assert_eq!(stop["breakpoint"], counted, "{stop}");
    assert_eq!(stop["hits"], 3, "{stop}");
    let params = json!({ "subscription": sub, "breakpoint": counted, "count": 1 });
    assert_valid(
        &method_def("debug.ignore")["params"],
        &params,
        "debug.ignore params",
    );
    c.call("debug.ignore", params).await.unwrap();
    c.call(
        "debug.control",
        json!({ "subscription": sub, "action": "continue" }),
    )
    .await
    .unwrap();
    let stop = debug_wait(&mut c, &sub, "debug.stopped").await;
    assert_eq!(stop["hits"], 5, "{stop}");
    c.call(
        "debug.delete",
        json!({ "subscription": sub, "breakpoint": counted }),
    )
    .await
    .unwrap();
    match c
        .call(
            "debug.break",
            json!({ "subscription": sub, "location": "setup", "condition": "$_shell(\"x\")" }),
        )
        .await
    {
        Err(ClientError::Rpc(e)) => assert_eq!(e.code, code::INVALID_PARAMS),
        other => panic!("$_shell condition accepted: {other:?}"),
    }

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
    // It runs on: the firmware ticks (it printed its ready line before the run to
    // the tick's line).
    let mut seen = String::new();
    // One deadline for the loop: other events keep arriving, so a per-event
    // timeout would never fire.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(90);
    while !seen.contains("esp32bench-hello tick") {
        let n = tokio::time::timeout_at(deadline, c.next_notification())
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
    // A recording of the panel: the picture there now, as an animated PNG.
    let png = d.path().join("panel.png");
    let r = c
        .call("sim.record", json!({ "subscription": sub, "path": png }))
        .await
        .unwrap();
    assert_valid(&method_def("sim.record")["result"], &r, "sim.record result");
    let r = c
        .call("sim.record", json!({ "subscription": sub }))
        .await
        .unwrap();
    assert_valid(&method_def("sim.record")["result"], &r, "sim.record result");
    assert!(r["frames"].as_u64().unwrap() >= 1, "{r}");
    let data = std::fs::read(&png).unwrap();
    assert!(data.starts_with(b"\x89PNG") && data.windows(4).any(|w| w == b"acTL"));
    match c.call("sim.record", json!({ "subscription": sub })).await {
        Err(ClientError::Rpc(e)) => assert_eq!(e.code, code::FAILED),
        other => panic!("stopped a recording that was not going: {other:?}"),
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

    // GPIO 2 toggles: seen high and low, as an output, and in the trace with times.
    let (mut high, mut low) = (false, false);
    let mut text = String::new();
    let mut traced: Vec<Value> = Vec::new();
    // One deadline for the loop: other events keep arriving, so a per-event
    // timeout would never fire.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(60);
    while !(high && low && traced.iter().filter(|e| e["gpio"] == 2).count() >= 2) {
        let n = tokio::time::timeout_at(deadline, c.next_notification())
            .await
            .expect("GPIO 2 did not toggle")
            .unwrap();
        let method = n["method"].as_str().unwrap();
        assert_valid(&method_def(method)["params"], &n["params"], method);
        if method == "sim.output" {
            text.push_str(n["params"]["text"].as_str().unwrap());
        }
        if method == "sim.trace" {
            traced.extend(n["params"]["edges"].as_array().unwrap().iter().cloned());
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
    // Its rate: the firmware flips it every 500 ms, so about 1 Hz, half the time high.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
    let rate = loop {
        let n = tokio::time::timeout_at(deadline, c.next_notification())
            .await
            .expect("no rate for GPIO 2")
            .unwrap();
        let method = n["method"].as_str().unwrap();
        assert_valid(&method_def(method)["params"], &n["params"], method);
        match method {
            "sim.output" => text.push_str(n["params"]["text"].as_str().unwrap()),
            "sim.trace" => traced.extend(n["params"]["edges"].as_array().unwrap().iter().cloned()),
            "sim.pins" => {
                let found = n["params"]["pins"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|p| p["gpio"] == 2 && p["rate"].is_object())
                    .map(|p| p["rate"].clone());
                if let Some(r) = found {
                    break r;
                }
            }
            _ => {}
        }
    };
    let (hz, high) = (rate["hz"].as_f64().unwrap(), rate["high"].as_f64().unwrap());
    assert!((0.8..=1.05).contains(&hz), "{rate}");
    assert!((0.4..=0.6).contains(&high), "{rate}");
    // Drive GPIO 4; the firmware reports what it reads.
    c.call(
        "sim.setPin",
        json!({ "subscription": sub, "gpio": 4, "level": true }),
    )
    .await
    .unwrap();
    let input_traced = |t: &[Value]| t.iter().any(|e| e["gpio"] == 4 && e["input"] == true);
    // One deadline for the loop: other events keep arriving, so a per-event
    // timeout would never fire.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
    while !(text.contains("esp32bench-gpio in 4=1") && input_traced(&traced)) {
        let n = tokio::time::timeout_at(deadline, c.next_notification())
            .await
            .unwrap_or_else(|_| panic!("firmware never read GPIO 4 high: {text}"))
            .unwrap();
        if n["method"] == "sim.output" {
            text.push_str(n["params"]["text"].as_str().unwrap());
        }
        if n["method"] == "sim.trace" {
            traced.extend(n["params"]["edges"].as_array().unwrap().iter().cloned());
        }
    }
    let times: Vec<u64> = traced.iter().map(|e| e["t"].as_u64().unwrap()).collect();
    assert!(times.windows(2).all(|w| w[0] <= w[1]), "{times:?}");
    // A pulse: GPIO 4 low for 300 ms, then high again by itself.
    let params = json!({ "subscription": sub, "gpio": 4, "level": false, "ms": 300 });
    assert_valid(
        &method_def("sim.pulsePin")["params"],
        &params,
        "sim.pulsePin params",
    );
    c.call("sim.pulsePin", params).await.unwrap();
    let from = text.len();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let after = &text[from..];
        if after
            .find("esp32bench-gpio in 4=0")
            .is_some_and(|low| after[low..].contains("esp32bench-gpio in 4=1"))
        {
            break;
        }
        let n = tokio::time::timeout_at(deadline, c.next_notification())
            .await
            .unwrap_or_else(|_| panic!("the pulse did not go low and back: {}", &text[from..]))
            .unwrap();
        if n["method"] == "sim.output" {
            text.push_str(n["params"]["text"].as_str().unwrap());
        }
    }
    match c
        .call(
            "sim.pulsePin",
            json!({ "subscription": sub, "gpio": 4, "level": false, "ms": 0 }),
        )
        .await
    {
        Err(ClientError::Rpc(e)) => assert_eq!(e.code, code::INVALID_PARAMS),
        other => panic!("a 0 ms pulse accepted: {other:?}"),
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
    // The trace is still there once the simulation has ended.
    let vcd = d.path().join("blink.vcd");
    let params = json!({ "subscription": sub, "path": vcd });
    assert_valid(
        &method_def("sim.traceSave")["params"],
        &params,
        "sim.traceSave params",
    );
    let r = c.call("sim.traceSave", params).await.unwrap();
    assert_valid(
        &method_def("sim.traceSave")["result"],
        &r,
        "sim.traceSave result",
    );
    assert!(r["edges"].as_u64().unwrap() >= 3, "{r}");
    let text = std::fs::read_to_string(&vcd).unwrap();
    assert!(text.contains("$timescale 1 ns $end"), "{text}");
    assert!(
        text.contains(" GPIO2 $end") && text.contains(" GPIO4 $end"),
        "{text}"
    );
}

#[tokio::test]
async fn sim_trace_save_checks_its_arguments() {
    let (_d, sock, _lock) = daemon().await;
    let mut c = Client::connect(&sock).await.unwrap();
    match c
        .call(
            "sim.traceSave",
            json!({ "subscription": 99, "path": "/tmp/x.vcd" }),
        )
        .await
    {
        Err(ClientError::Rpc(e)) => assert_eq!(e.code, code::NOT_FOUND),
        other => panic!("saved the trace of no simulation: {other:?}"),
    }
    match c
        .call(
            "sim.traceSave",
            json!({ "subscription": 99, "path": "x.vcd" }),
        )
        .await
    {
        Err(ClientError::Rpc(e)) => assert_eq!(e.code, code::INVALID_PARAMS),
        other => panic!("accepted a relative path: {other:?}"),
    }
}

#[tokio::test]
async fn circuit_suggest_reads_the_wiring() {
    let (_d, sock, _lock) = daemon().await;
    let mut c = Client::connect(&sock).await.unwrap();
    let params = json!({ "project": "blink", "board": "esp32" });
    assert_valid(
        &method_def("circuit.suggest")["params"],
        &params,
        "circuit.suggest params",
    );
    // No library yet: nothing to read, so nothing suggested.
    let r = c.call("circuit.suggest", params.clone()).await.unwrap();
    assert_eq!(r["circuit"]["parts"], json!([]));
    c.call("library.refresh", json!({})).await.unwrap();
    c.next_notification().await.unwrap();

    let r = c.call("circuit.suggest", params).await.unwrap();
    assert_valid(
        &method_def("circuit.suggest")["result"],
        &r,
        "circuit.suggest result",
    );
    // The example's blink wires an LED to GPIO 2.
    assert_eq!(
        r["circuit"]["parts"],
        json!([{ "id": "led-1", "kind": "led", "pin": 2, "label": "LED" }])
    );
    // What it suggests is a circuit circuit.set takes; nothing was saved by asking.
    let got = c
        .call(
            "circuit.get",
            json!({ "project": "blink", "board": "esp32" }),
        )
        .await
        .unwrap();
    assert_eq!(got["circuit"]["parts"], json!([]));
    c.call(
        "circuit.set",
        json!({ "project": "blink", "board": "esp32", "circuit": r["circuit"] }),
    )
    .await
    .unwrap();
    // A project the library does not have gets none.
    let r = c
        .call(
            "circuit.suggest",
            json!({ "project": "local-3f9a0c21b4de", "board": "esp32" }),
        )
        .await
        .unwrap();
    assert_eq!(r["circuit"]["parts"], json!([]));
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
    // One deadline for the loop: other events keep arriving, so a per-event
    // timeout would never fire.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(60);
    while !(led.contains(&true) && led.contains(&false) && led.len() >= 3) {
        let n = tokio::time::timeout_at(deadline, c.next_notification())
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
    // One deadline for the loop: other events keep arriving, so a per-event
    // timeout would never fire.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
    while !(text.contains("esp32bench-gpio in 4=1") && pressed_seen) {
        let n = tokio::time::timeout_at(deadline, c.next_notification())
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
    assert_eq!(r["circuit"]["schemaVersion"], "1.3.0");
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

// A file at a line in the user's editor: a template run without a shell, and refusals.
#[cfg(unix)]
#[tokio::test]
async fn editor_open_runs_the_template_and_refuses_bad_ones() {
    let (d, sock, _lock) = daemon().await;
    let mut c = Client::connect(&sock).await.unwrap();
    let file = d.path().join("main.c");
    std::fs::write(&file, "int main;\n").unwrap();
    let params =
        json!({ "path": file, "line": 1, "column": 5, "editor": "true {file}:{line}:{col}" });
    assert_valid(
        &method_def("editor.open")["params"],
        &params,
        "editor.open params",
    );
    let r = c.call("editor.open", params).await.unwrap();
    assert_valid(
        &method_def("editor.open")["result"],
        &r,
        "editor.open result",
    );
    for (params, want) in [
        (
            json!({ "path": d.path().join("missing.c"), "editor": "true {file}" }),
            code::INVALID_PARAMS,
        ),
        (
            json!({ "path": file, "editor": "true" }),
            code::INVALID_PARAMS,
        ),
        (json!({ "path": "main.c" }), code::INVALID_PARAMS),
        (
            json!({ "path": file, "editor": "no-such-editor-esp32bench {file}" }),
            code::FAILED,
        ),
    ] {
        match c.call("editor.open", params.clone()).await {
            Err(ClientError::Rpc(e)) => assert_eq!(e.code, want, "{params}: {}", e.message),
            other => panic!("{params}: {other:?}"),
        }
    }
}

/// The user's own Arduino sketch (phase 7): local.open takes the folder or its .ino,
/// local.get lists the library's boards and the sketch's files, and build.start builds
/// it in place without asking for consent (the user chose it), its outputs where the
/// simulator and flashing look for a project's build.
#[cfg(unix)]
#[tokio::test]
async fn arduino_sketches_open_and_build_in_place() {
    let (d, sock, _lock) = daemon().await;
    let mut c = Client::connect(&sock).await.unwrap();
    with_library(&mut c).await;
    let sketch = d.path().join("Blinky");
    std::fs::create_dir_all(&sketch).unwrap();
    std::fs::write(
        sketch.join("Blinky.ino"),
        "void setup() {}\nvoid loop() {}\n",
    )
    .unwrap();
    std::fs::write(sketch.join("pins.h"), "#define LED 2\n").unwrap();
    let r = c
        .call("local.open", json!({ "path": sketch.join("Blinky.ino") }))
        .await
        .unwrap();
    assert_valid(&method_def("local.open")["result"], &r, "local.open result");
    assert_eq!(r["project"]["kind"], "arduino");
    assert_eq!(r["project"]["name"], "Blinky");
    let id = r["project"]["id"].as_str().unwrap().to_owned();
    let got = c.call("local.get", json!({ "id": id })).await.unwrap();
    assert_valid(&method_def("local.get")["result"], &got, "local.get result");
    assert_eq!(got["sources"], json!(["Blinky.ino", "pins.h"]));
    assert_eq!(got["exists"], true);
    let boards: Vec<&str> = got["boards"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["id"].as_str().unwrap())
        .collect();
    assert!(
        boards.contains(&"esp32c3-supermini") && boards.contains(&"esp32"),
        "{boards:?}"
    );

    let r = c
        .call("build.start", json!({ "project": id, "board": "esp32" }))
        .await
        .unwrap();
    assert_valid(
        &method_def("build.start")["result"],
        &r,
        "build.start result",
    );
    assert_eq!(r["plan"]["fqbn"], "esp32:esp32:esp32:FlashMode=dio");
    let done = loop {
        let n = tokio::time::timeout(std::time::Duration::from_secs(10), c.next_notification())
            .await
            .unwrap()
            .unwrap();
        let event = n["method"].as_str().unwrap().to_owned();
        assert_valid(&method_def(&event)["params"], &n["params"], &event);
        if event == "build.finished" {
            break n["params"].clone();
        }
    };
    assert_eq!(done["ok"], true, "{done}");
    let image = std::path::PathBuf::from(done["image"].as_str().unwrap());
    assert_eq!(
        image.file_name().unwrap().to_string_lossy(),
        format!("{id}.ino.merged.bin")
    );
    assert!(image.is_file());
    let status = c
        .call("build.status", json!({ "project": id, "board": "esp32" }))
        .await
        .unwrap();
    assert_eq!(status["built"], true, "{status}");
    // Nothing was written into the user's folder.
    let mut names: Vec<String> = std::fs::read_dir(&sketch)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(names, ["Blinky.ino", "pins.h"]);

    // clangd (7.3): a .clangd pointing at the build's database, mapped to the sketch.
    let r = c
        .call("clangd.setup", json!({ "project": id, "board": "esp32" }))
        .await
        .unwrap();
    assert_valid(
        &method_def("clangd.setup")["result"],
        &r,
        "clangd.setup result",
    );
    let dot = std::fs::read_to_string(sketch.join(".clangd")).unwrap();
    assert!(dot.starts_with("# Written by ESP32 Bench"), "{dot}");
    let db_dir = std::path::PathBuf::from(r["compilationDatabase"].as_str().unwrap());
    let db: Value =
        serde_json::from_slice(&std::fs::read(db_dir.join("compile_commands.json")).unwrap())
            .unwrap();
    assert_eq!(
        db[0]["file"],
        sketch
            .canonicalize()
            .unwrap()
            .join("Blinky.ino")
            .to_string_lossy()
            .as_ref()
    );
    assert!(
        r["queryDriver"].as_str().unwrap().ends_with("/**/bin/*"),
        "{r}"
    );
    // A .clangd of the user's own is left alone; a board never built is refused.
    std::fs::write(sketch.join(".clangd"), "CompileFlags:\n  Add: [-DMINE]\n").unwrap();
    for (board, why) in [("esp32", "already there"), ("esp32s3", "once first")] {
        match c
            .call("clangd.setup", json!({ "project": id, "board": board }))
            .await
        {
            Err(ClientError::Rpc(e)) => assert!(e.message.contains(why), "{}", e.message),
            other => panic!("{board}: {other:?}"),
        }
    }
    assert_eq!(
        std::fs::read_to_string(sketch.join(".clangd")).unwrap(),
        "CompileFlags:\n  Add: [-DMINE]\n"
    );
}
