//! The virtual RGB panel through screendump polling, on every engine installed.
//! Needs `tests/firmware/build.sh rgb` and `... hello`; skips what is missing.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use esp32bench_sim::display::{Display, Frame};
use esp32bench_sim::{Chip, Engine, Session, engine};

fn image(project: &str, chip: Chip) -> Option<PathBuf> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(format!("../../../tests/firmware/out/{project}-{chip}.bin"));
    p.is_file().then_some(p)
}

/// Run `project` for `secs`, polling the panel as the daemon does. Returns the frames
/// and UART0 text.
async fn run(
    launch: &esp32bench_sim::Launch,
    chip: Chip,
    project: &str,
    secs: u64,
    stop: impl Fn(&[Frame]) -> bool,
) -> (Vec<Frame>, String) {
    let mut s = Session::start(launch.config(chip, image(project, chip).unwrap()))
        .await
        .unwrap();
    let mut display = Display::new(&s.paths().dir.clone());
    let mut frames = Vec::new();
    let mut text = Vec::new();
    let end = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < end && !stop(&frames) {
        tokio::select! {
            () = tokio::time::sleep(display.interval()) => {
                if let Some(f) = display.poll(&mut s.qmp).await.unwrap() {
                    frames.push(f);
                }
            }
            chunk = s.uart.read_chunk() => {
                if let Ok(Some(c)) = chunk { text.extend_from_slice(&c) }
            }
        }
    }
    s.shutdown().await.unwrap();
    (frames, String::from_utf8_lossy(&text).into_owned())
}

const RED: [u8; 3] = [255, 0, 0];
const GREEN: [u8; 3] = [0, 255, 0];
const BLUE: [u8; 3] = [0, 0, 255];
const WHITE: [u8; 3] = [255, 255, 255];
const BLACK: [u8; 3] = [0, 0, 0];

fn quadrants(f: &Frame) -> [Option<[u8; 3]>; 4] {
    [
        f.pixel(10, 10),
        f.pixel(250, 10),
        f.pixel(10, 200),
        f.pixel(250, 200),
    ]
}

async fn panel_shows_and_updates(chip: Chip) {
    if image("rgb", chip).is_none() {
        eprintln!("skipping {chip}: rgb firmware not built");
        return;
    }
    for e in [Engine::Lcgamboa, Engine::Espressif] {
        let Ok(launch) = engine::launch(chip, Some(e)) else {
            eprintln!("skipping {chip} on {}: not installed", e.as_str());
            continue;
        };
        let full = |f: &Frame| quadrants(f) == [Some(RED), Some(GREEN), Some(BLUE), Some(WHITE)];
        let toggled =
            |f: &Frame| quadrants(f) == [Some(BLACK), Some(GREEN), Some(BLUE), Some(WHITE)];
        let (frames, text) = run(&launch, chip, "rgb", 20, |fs| {
            fs.iter()
                .position(full)
                .is_some_and(|i| fs[i..].iter().any(toggled))
        })
        .await;
        let i = frames.iter().position(full).unwrap_or_else(|| {
            panic!(
                "{chip} on {}: no full picture in {} frames; uart: {text}",
                e.as_str(),
                frames.len()
            )
        });
        assert!(
            frames[i..].iter().any(toggled),
            "{chip} on {}: picture never changed",
            e.as_str()
        );
        assert_eq!((frames[i].width, frames[i].height), (320, 240));
        // Its draws returned: it prints this after drawing all four quadrants.
        assert!(
            text.contains("esp32bench-rgb ready 320x240"),
            "{chip} on {}: {text}",
            e.as_str()
        );
    }
}

#[tokio::test]
async fn esp32_panel_shows_and_updates() {
    panel_shows_and_updates(Chip::Esp32).await;
}

#[tokio::test]
async fn esp32s3_panel_shows_and_updates() {
    panel_shows_and_updates(Chip::Esp32s3).await;
}

#[tokio::test]
async fn esp32c3_panel_shows_and_updates() {
    panel_shows_and_updates(Chip::Esp32c3).await;
}

#[tokio::test]
async fn firmware_without_a_display_shows_none() {
    let chip = Chip::Esp32;
    if image("hello", chip).is_none() {
        eprintln!("skipping: hello firmware not built");
        return;
    }
    let Ok(launch) = engine::launch(chip, None) else {
        eprintln!("skipping: no QEMU");
        return;
    };
    let (frames, text) = run(&launch, chip, "hello", 3, |_| false).await;
    assert!(text.contains("esp32bench-hello ready"), "{text}");
    assert!(
        frames.is_empty(),
        "{} frames from a firmware with no display",
        frames.len()
    );
}
