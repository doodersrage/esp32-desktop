//! GDB against QEMU's stub, on the hello firmware. Skips without QEMU, the pinned
//! Arduino core's GDB, or the firmware (tests/firmware/build.sh).

use std::path::{Path, PathBuf};
use std::time::Duration;

use esp32bench_debug::{Event, Gdb, find_gdb};
use esp32bench_sim::Engine;
use tokio::sync::mpsc::UnboundedReceiver;

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

async fn stopped(rx: &mut UnboundedReceiver<Event>) -> serde_json::Value {
    loop {
        match tokio::time::timeout(Duration::from_secs(30), rx.recv()).await {
            Ok(Some(Event::Stopped(v))) => {
                eprintln!("stopped: {v}");
                return v;
            }
            Ok(Some(Event::Exited)) | Ok(None) => panic!("GDB exited"),
            Ok(Some(e)) => eprintln!("event: {e:?}"),
            Err(_) => panic!("no stop within 30 s"),
        }
    }
}

#[tokio::test]
async fn esp32_breaks_in_app_main_and_steps() {
    breaks_in_app_main_and_steps(esp32bench_sim::Chip::Esp32, "esp32").await;
}

#[tokio::test]
async fn esp32s3_breaks_in_app_main_and_steps() {
    breaks_in_app_main_and_steps(esp32bench_sim::Chip::Esp32s3, "esp32s3").await;
}

#[tokio::test]
async fn esp32c3_breaks_in_app_main_and_steps() {
    breaks_in_app_main_and_steps(esp32bench_sim::Chip::Esp32c3, "esp32c3").await;
}

async fn breaks_in_app_main_and_steps(chip: esp32bench_sim::Chip, name: &str) {
    let elf = repo().join(format!(
        "tests/firmware/build/hello-{name}/esp32bench-hello.elf"
    ));
    let image = repo().join(format!("tests/firmware/out/hello-{name}.bin"));
    let data = std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join(".arduino15"))
        .unwrap_or_default();
    let Some(gdb) = find_gdb(&data, name) else {
        eprintln!("skipping: needs the Arduino core's GDB");
        return;
    };
    if !elf.is_file() || !image.is_file() {
        eprintln!("skipping: run tests/firmware/build.sh first");
        return;
    }
    // On every engine installed here.
    for engine in [Engine::Espressif, Engine::Lcgamboa] {
        let Ok(launch) = esp32bench_sim::engine::launch(chip, Some(engine)) else {
            eprintln!("skipping {name} on {}: not installed", engine.as_str());
            continue;
        };
        eprintln!("{name} on {}", engine.as_str());
        debug_session(launch, chip, &gdb, &elf, image.clone()).await;
    }
}

async fn debug_session(
    launch: esp32bench_sim::Launch,
    chip: esp32bench_sim::Chip,
    gdb: &Path,
    elf: &Path,
    image: PathBuf,
) {
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let mut config = launch.config(chip, image);
    config.gdb_port = Some(port);
    config.start_paused = true;
    let mut session = esp32bench_sim::Session::start(config).await.unwrap();
    // Keep UART0 drained, as the daemon does; the task owns QEMU until the test ends.
    let qemu_task = tokio::spawn(async move {
        while let Ok(Some(b)) = session.uart.read_chunk().await {
            let _ = b;
        }
    });
    let (mut g, mut rx) = Gdb::start(gdb, elf).await.unwrap();
    g.connect(port).await.unwrap();
    let bkpt = g.break_insert("app_main", None).await.unwrap();
    assert_eq!(bkpt["func"], "app_main");
    g.control("continue").await.unwrap();
    let stop = stopped(&mut rx).await;
    // The first stop may be the attach itself.
    let stop = if stop["reason"] == "breakpoint-hit" {
        stop
    } else {
        stopped(&mut rx).await
    };
    assert_eq!(stop["reason"], "breakpoint-hit");
    assert_eq!(stop["frame"]["func"], "app_main");
    g.control("next").await.unwrap();
    let stop = stopped(&mut rx).await;
    assert_eq!(stop["reason"], "end-stepping-range");
    let frames = g.frames().await.unwrap();
    assert_eq!(frames[0]["func"], "app_main");
    let regs = g.registers().await.unwrap();
    assert!(regs.iter().any(|(n, v)| n == "pc" && v.starts_with("0x")));
    assert!(regs.iter().any(|(n, _)| n == "sp" || n == "a1"), "{regs:?}");
    // app_main's locals: the chip info struct and the line buffer have no simple
    // value; they are listed with their types.
    let locals = g.locals().await.unwrap();
    let info = locals.iter().find(|l| l.name == "info").expect("no `info`");
    assert_eq!(
        (info.kind.as_str(), info.arg, &info.value),
        ("esp_chip_info_t", false, &None),
        "{locals:?}"
    );
    assert!(
        locals
            .iter()
            .any(|l| l.name == "line" && l.kind.starts_with("char [")),
        "{locals:?}"
    );
    // Interrupt works while running (mi-async).
    g.break_delete(1).await.unwrap();
    g.control("continue").await.unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    g.control("interrupt").await.unwrap();
    let stop = stopped(&mut rx).await;
    assert_eq!(stop["reason"], "signal-received");
    assert!(
        g.command("-break-insert nope_not_a_function")
            .await
            .is_err()
    );
    qemu_task.abort();
}
