//! Build plans, consent and runs. Runs use a stand-in arduino-cli script, so no
//! toolchain or network is needed. Set ESP32BENCH_TEST_ARDUINO=1 to also compile for
//! real with the installed arduino-cli (downloads libraries).

use std::path::{Path, PathBuf};
use std::time::Duration;

use esp32bench_build::{Consent, Dirs, Error, Stream, Toolchain, plan, run};
use esp32bench_library::{Manifest, parse};

fn example() -> Manifest {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../schema/project-manifest/example.json");
    parse(&std::fs::read(p).unwrap()).unwrap()
}

#[test]
fn plans_a_sketch_build() {
    let m = example();
    let (p, code) = plan(&m, "blink", "esp32").unwrap();
    assert_eq!(p.target, "esp32");
    assert_eq!(p.fqbn, "esp32:esp32:esp32:FlashMode=dio");
    assert_eq!(p.install, ["Adafruit NeoPixel"]);
    assert_eq!(p.code_sha256.len(), 64);
    assert!(code.contains("esp32"));
}

#[test]
fn variant_boards_build_as_their_parent() {
    let (p, _) = plan(&example(), "blink", "esp32-doit").unwrap();
    assert_eq!(p.board, "esp32-doit");
    assert_eq!(p.target, "esp32");
    assert!(p.fqbn.starts_with("esp32:esp32:esp32:"));
}

#[test]
fn refuses_what_cannot_be_built() {
    let mut m = example();
    // Prebuilt firmware with an image (manifest 1.1) is planned as a download of
    // exactly that image; without one there is nothing to do.
    let (p, code) = plan(&m, "probeharbor-probe", "esp32").unwrap();
    let image = p.image.clone().unwrap();
    assert_eq!(p.code_sha256, image.sha256);
    assert!(p.fqbn.is_empty() && p.install.is_empty() && code.is_empty());
    for proj in &mut m.projects {
        for b in &mut proj.builds {
            b.image = None;
        }
    }
    assert!(matches!(
        plan(&m, "probeharbor-probe", "esp32"),
        Err(Error::Prebuilt(_))
    ));
    assert!(matches!(
        plan(&m, "blink", "esp32c6"),
        Err(Error::NoBuild { .. })
    ));
    assert!(matches!(
        plan(&m, "nope", "esp32"),
        Err(Error::NoProject(_))
    ));
    assert!(matches!(plan(&m, "../etc", "esp32"), Err(Error::BadId(_))));
    assert!(matches!(
        plan(&m, "blink", "esp32/../../x"),
        Err(Error::BadId(_))
    ));
}

#[test]
fn huge_app_projects_get_a_3_mb_app_on_every_board() {
    let mut m = example();
    let blink = m.projects.iter_mut().find(|p| p.id == "blink").unwrap();
    blink.partitions = Some("huge_app".into());
    let mut xiao = blink.builds[0].clone();
    xiao.board = "xiao-esp32s3".into();
    blink.builds.push(xiao);

    let (p, _) = plan(&m, "blink", "esp32").unwrap();
    assert!(p.fqbn.ends_with(",PartitionScheme=huge_app"));
    // The XIAO ESP32-S3 has no huge_app entry; its 8 MB default has a 3.2 MB app.
    let (p, _) = plan(&m, "blink", "xiao-esp32s3").unwrap();
    assert!(
        p.fqbn.ends_with(",PartitionScheme=default_8MB"),
        "{}",
        p.fqbn
    );

    let blink = m.projects.iter_mut().find(|p| p.id == "blink").unwrap();
    blink.partitions = Some("min_spiffs".into());
    assert!(matches!(
        plan(&m, "blink", "esp32"),
        Err(Error::NoPartitionScheme { .. })
    ));
}

#[test]
fn consent_is_single_use_and_for_one_exact_plan() {
    let m = example();
    let (p, _) = plan(&m, "blink", "esp32").unwrap();
    let (other, _) = plan(&m, "blink", "esp32-doit").unwrap();
    let c = Consent::new();

    let t = c.issue(&p).unwrap();
    assert_eq!(t.len(), 32);
    assert!(c.redeem(&t, &p));
    assert!(!c.redeem(&t, &p), "a token worked twice");

    let t = c.issue(&p).unwrap();
    assert!(!c.redeem(&t, &other), "a token worked for another plan");
    assert!(!c.redeem(&t, &p), "a token survived a failed redeem");

    let mut changed = p.clone();
    changed.code_sha256 = "0".repeat(64);
    let t = c.issue(&p).unwrap();
    assert!(
        !c.redeem(&t, &changed),
        "a token worked after the code changed"
    );

    let t = c.issue(&p).unwrap();
    c.age_for_test(&t, Duration::from_secs(11 * 60));
    assert!(!c.redeem(&t, &p), "an expired token worked");
    assert!(!c.redeem("not-a-token", &p));
}

/// How the stand-in arduino-cli behaves.
enum Fake {
    Works,
    Fails,
    /// Starts a child process, writes its pid to this file, and waits for it.
    Hangs(PathBuf),
}

/// A stand-in arduino-cli for one test. It records its arguments and
/// ARDUINO_DIRECTORIES_USER in `log`, prints to both streams and writes the outputs.
/// Everything it needs is written into the script, so tests share no state.
fn fake_cli(dir: &Path, log: &Path, mode: Fake) -> PathBuf {
    let path = dir.join("arduino-cli");
    let behaviour = match mode {
        Fake::Works => String::new(),
        Fake::Fails => "echo 'error: it broke' >&2; exit 1".to_owned(),
        Fake::Hangs(pidfile) => format!("sleep 60 & echo $! > '{}'; wait", pidfile.display()),
    };
    let script = format!(
        r#"#!/bin/sh
echo "args: $*" >> '{log}'
echo "user: $ARDUINO_DIRECTORIES_USER" >> '{log}'
case "$1" in
  lib) echo "Downloading $3"; echo "Installed $3"; exit 0 ;;
  compile)
    out=""; prev=""
    for a in "$@"; do [ "$prev" = "--output-dir" ] && out="$a"; prev="$a"; last="$a"; done
    name=$(basename "$last")
    echo "Sketch uses 1234 bytes"
    echo "warning: something minor" >&2
    {behaviour}
    : > "$out/$name.ino.merged.bin"; : > "$out/$name.ino.elf"
    ;;
esac
"#,
        log = log.display(),
    );
    write_script(&path, script);
    path
}

struct Fixture {
    _dir: tempfile::TempDir,
    tools: Toolchain,
    dirs: Dirs,
    log: PathBuf,
    plan: esp32bench_build::Plan,
    code: String,
}

fn fixture(mode: impl FnOnce(&Path) -> Fake) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("cli.log");
    let tools = Toolchain {
        arduino_cli: fake_cli(dir.path(), &log, mode(dir.path())),
        sketchbook: dir.path().join("sketchbook"),
    };
    let (plan, code) = plan(&example(), "blink", "esp32").unwrap();
    let dirs = Dirs::new(&dir.path().join("builds"), &plan);
    Fixture {
        tools,
        dirs,
        log,
        plan,
        code,
        _dir: dir,
    }
}

#[tokio::test]
async fn runs_install_then_compile_and_streams_output() {
    let f = fixture(|_| Fake::Works);
    let mut lines = Vec::new();
    let out = run(&f.plan, &f.code, &f.tools, &f.dirs, |s, l| {
        lines.push((s, l))
    })
    .await
    .unwrap();

    assert!(out.image.is_file() && out.elf.is_file());
    assert_eq!(
        std::fs::read_to_string(f.dirs.ino(&f.plan)).unwrap(),
        f.code
    );
    assert!(lines.contains(&(Stream::Stdout, "Installed Adafruit NeoPixel".into())));
    assert!(lines.contains(&(Stream::Stderr, "warning: something minor".into())));

    let log = std::fs::read_to_string(&f.log).unwrap();
    let calls: Vec<&str> = log.lines().filter(|l| l.starts_with("args: ")).collect();
    assert_eq!(calls[0], "args: lib install Adafruit NeoPixel");
    assert!(
        calls[1].starts_with("args: compile --fqbn esp32:esp32:esp32:FlashMode=dio --build-path ")
    );
    // Libraries go into the private sketchbook, not ~/Arduino.
    assert!(log.contains(&format!("user: {}", f.tools.sketchbook.display())));
}

#[tokio::test]
async fn reports_a_failed_compile() {
    let f = fixture(|_| Fake::Fails);
    let mut lines = Vec::new();
    let r = run(&f.plan, &f.code, &f.tools, &f.dirs, |s, l| {
        lines.push((s, l))
    })
    .await;
    assert!(
        matches!(
            r,
            Err(Error::StepFailed {
                step: "compiling",
                ..
            })
        ),
        "{r:?}"
    );
    assert!(lines.contains(&(Stream::Stderr, "error: it broke".into())));
}

#[tokio::test]
async fn cancelling_kills_the_whole_process_group() {
    let f = fixture(|d| Fake::Hangs(d.join("grandchild.pid")));
    let pidfile = f.log.with_file_name("grandchild.pid");
    let build = run(&f.plan, &f.code, &f.tools, &f.dirs, |_, _| {});
    // Let it reach the hanging compile, then cancel by dropping the future.
    let r = tokio::time::timeout(Duration::from_secs(2), build).await;
    assert!(r.is_err(), "the hanging build finished");
    let pid: i32 = std::fs::read_to_string(&pidfile)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    for _ in 0..50 {
        // A killed child may linger as a zombie until reaped; count that as gone.
        let state = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
        if state.is_empty() || state.split_whitespace().nth(2) == Some("Z") {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the compiler's child process {pid} survived cancellation");
}

#[tokio::test]
async fn compiles_for_real_when_asked() {
    if std::env::var_os("ESP32BENCH_TEST_ARDUINO").is_none() {
        eprintln!("skipping: set ESP32BENCH_TEST_ARDUINO=1 to compile with the real arduino-cli");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let tools = Toolchain::detect(dir.path().join("sketchbook")).unwrap();
    let (p, code) = plan(&example(), "blink", "esp32").unwrap();
    let dirs = Dirs::new(&dir.path().join("builds"), &p);
    let out = run(&p, &code, &tools, &dirs, |s, l| eprintln!("{s:?}: {l}"))
        .await
        .unwrap();
    // QEMU needs a 4 MiB DIO image; the merged image is padded to the flash size.
    assert_eq!(
        std::fs::metadata(&out.image).unwrap().len(),
        4 * 1024 * 1024
    );
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
