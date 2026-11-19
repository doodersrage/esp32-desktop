//! QEMU boot tests. They need the pinned QEMU (`esp32bench qemu install`) and a merged
//! image per chip. By default that is `tests/firmware/out/hello-<chip>.bin`, built by
//! `tests/firmware/build.sh`, and the test waits for its ready line. Set
//! `ESP32BENCH_TEST_IMAGE_<CHIP>` (for example `ESP32BENCH_TEST_IMAGE_ESP32C3=/path/flash.bin`)
//! to boot another image; the test then only waits for the ROM's `rst:0x`.
//! Without QEMU or an image, each test is skipped.

use std::path::PathBuf;
use std::time::Duration;

use esp32bench_sim::uart::WaitOutcome;
use esp32bench_sim::{Chip, Session, SessionConfig, gdb, install};

const HELLO_MARKER: &[u8] = b"esp32bench-hello ready";
const ROM_MARKER: &[u8] = b"rst:0x";

/// The image to boot and the UART0 line that proves it booted.
fn image_for(chip: Chip) -> Option<(PathBuf, &'static [u8])> {
    let var = format!(
        "ESP32BENCH_TEST_IMAGE_{}",
        chip.machine().to_ascii_uppercase()
    );
    if let Some(image) = std::env::var_os(&var) {
        return Some((image.into(), ROM_MARKER));
    }
    let hello = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../tests/firmware/out")
        .join(format!("hello-{chip}.bin"));
    if hello.is_file() {
        return Some((hello, HELLO_MARKER));
    }
    eprintln!(
        "skipping {chip}: no {} (run tests/firmware/build.sh) and {var} not set",
        hello.display()
    );
    None
}

fn setup(chip: Chip) -> Option<(SessionConfig, &'static [u8])> {
    let (image, marker) = image_for(chip)?;
    let qemu = match install::find_qemu(chip.arch()) {
        Ok(q) => q,
        Err(e) => {
            eprintln!("skipping {chip}: {e}");
            return None;
        }
    };
    let cfg = SessionConfig {
        chip,
        qemu,
        image,
        gdb_port: None,
        start_paused: false,
        machine: None,
        extra_args: Vec::new(),
        env: Vec::new(),
        bridge: Vec::new(),
        circuit: None,
        efuse: None,
        usb_console: false,
        network: None,
        icount: false,
    };
    Some((cfg, marker))
}

async fn boot_reset_and_quit(chip: Chip) {
    let Some((cfg, marker)) = setup(chip) else {
        return;
    };
    boot_reset_and_quit_with(cfg, marker).await;
}

/// The same on lcgamboa's fork, as `toolchain install qemu-lcgamboa` builds it.
async fn lcgamboa_boot_reset_and_quit(chip: Chip) {
    let Some(build) = esp32bench_sim::lcgamboa::Build::find() else {
        eprintln!("skipping {chip}: lcgamboa's QEMU is not built");
        return;
    };
    let Some((image, marker)) = image_for(chip) else {
        return;
    };
    let cfg = SessionConfig {
        chip,
        qemu: build.executable(chip.arch()),
        image,
        gdb_port: None,
        start_paused: false,
        machine: Some(esp32bench_sim::lcgamboa::machine(chip).to_owned()),
        extra_args: build.qemu_args(),
        env: Vec::new(),
        bridge: Vec::new(),
        circuit: None,
        efuse: None,
        usb_console: false,
        network: None,
        icount: false,
    };
    boot_reset_and_quit_with(cfg, marker).await;
}

#[tokio::test]
async fn lcgamboa_esp32_boots() {
    lcgamboa_boot_reset_and_quit(Chip::Esp32).await;
}

#[tokio::test]
async fn lcgamboa_esp32s3_boots() {
    lcgamboa_boot_reset_and_quit(Chip::Esp32s3).await;
}

#[tokio::test]
async fn lcgamboa_esp32c3_boots() {
    lcgamboa_boot_reset_and_quit(Chip::Esp32c3).await;
}

async fn boot_reset_and_quit_with(cfg: SessionConfig, marker: &[u8]) {
    let dir = {
        let mut s = Session::start(cfg).await.unwrap();
        let dir = s.paths().dir.clone();
        let t = Duration::from_secs(30);
        assert_eq!(
            s.uart.wait_for(marker, t, |_| {}).await.unwrap(),
            WaitOutcome::Found
        );
        s.reset().await.unwrap();
        assert_eq!(
            s.uart.wait_for(marker, t, |_| {}).await.unwrap(),
            WaitOutcome::Found
        );
        s.shutdown().await.unwrap();
        dir
    };
    assert!(!dir.exists(), "session directory left behind");
}

#[tokio::test]
async fn esp32_boots() {
    boot_reset_and_quit(Chip::Esp32).await;
}

#[tokio::test]
async fn esp32s3_boots() {
    boot_reset_and_quit(Chip::Esp32s3).await;
}

#[tokio::test]
async fn esp32c3_boots() {
    boot_reset_and_quit(Chip::Esp32c3).await;
}

/// A built firmware image from tests/firmware/out, if present.
fn built_image(project: &str, chip: Chip) -> Option<PathBuf> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../tests/firmware/out")
        .join(format!("{project}-{chip}.bin"));
    if p.is_file() {
        Some(p)
    } else {
        eprintln!("skipping {project} on {chip}: {} not built", p.display());
        None
    }
}

/// The Arduino sketch, built with the pinned ESP32 Arduino core, boots and resets.
#[tokio::test]
async fn arduino_hello_boots() {
    const READY: &[u8] = b"esp32bench-arduino ready";
    for chip in Chip::ALL {
        let Some(image) = built_image("hello-arduino", chip) else {
            continue;
        };
        let Ok(qemu) = install::find_qemu(chip.arch()) else {
            eprintln!("skipping hello-arduino on {chip}: QEMU not installed");
            continue;
        };
        let cfg = SessionConfig {
            chip,
            qemu,
            image,
            gdb_port: None,
            start_paused: false,
            machine: None,
            extra_args: Vec::new(),
            env: Vec::new(),
            bridge: Vec::new(),
            circuit: None,
            efuse: None,
            usb_console: false,
            network: None,
            icount: false,
        };
        let mut s = Session::start(cfg).await.unwrap();
        let t = Duration::from_secs(30);
        assert_eq!(
            s.uart.wait_for(READY, t, |_| {}).await.unwrap(),
            WaitOutcome::Found,
            "{chip}"
        );
        s.reset().await.unwrap();
        assert_eq!(
            s.uart.wait_for(READY, t, |_| {}).await.unwrap(),
            WaitOutcome::Found,
            "{chip} after reset"
        );
        s.shutdown().await.unwrap();
    }
}

/// The hello firmware echoes lines it receives on UART0.
#[tokio::test]
async fn uart_input_reaches_the_firmware() {
    for chip in Chip::ALL {
        let Some((cfg, marker)) = setup(chip) else {
            continue;
        };
        if marker != HELLO_MARKER {
            eprintln!("skipping UART input on {chip}: needs the hello firmware");
            continue;
        }
        let mut s = Session::start(cfg).await.unwrap();
        let t = Duration::from_secs(30);
        assert_eq!(
            s.uart.wait_for(marker, t, |_| {}).await.unwrap(),
            WaitOutcome::Found
        );
        s.uart.write(b"ping 42\r\n").await.unwrap();
        assert_eq!(
            s.uart
                .wait_for(b"esp32bench-hello echo ping 42", t, |_| {})
                .await
                .unwrap(),
            WaitOutcome::Found,
            "{chip} did not echo"
        );
        s.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn gdb_stub_answers_while_halted() {
    let Some((mut cfg, _)) = setup(Chip::Esp32) else {
        return;
    };
    // A port the OS says is free: a fixed one can be taken by a parallel test's
    // outgoing connection (it was: 41234 is in Linux's ephemeral range).
    cfg.gdb_port = Some(
        std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port(),
    );
    cfg.start_paused = true;
    let s = Session::start(cfg).await.unwrap();
    let reply = gdb::probe(s.gdb_addr().unwrap(), Duration::from_secs(5))
        .await
        .unwrap();
    assert!(
        reply.starts_with('T') || reply.starts_with('S'),
        "stop reply: {reply}"
    );
    s.shutdown().await.unwrap();
}

/// An Arduino sketch keeps running on lcgamboa's fork. Arduino switches the CPU to
/// 240 MHz at run time, and before patch 0003 that stopped a core's tick timer: about
/// half of runs panicked within seconds. ESP-IDF firmware does not exercise it, so
/// this needs an Arduino build. Repeated, since the failure depended on timing.
async fn arduino_keeps_running_on_lcgamboa(chip: Chip, runs: usize) {
    let Some(image) = built_image("hello-arduino", chip) else {
        eprintln!("skipping {chip}: hello-arduino not built");
        return;
    };
    let Ok(launch) = esp32bench_sim::engine::launch(chip, Some(esp32bench_sim::Engine::Lcgamboa))
    else {
        eprintln!("skipping {chip}: lcgamboa's QEMU is not built");
        return;
    };
    for run in 1..=runs {
        let mut s = Session::start(launch.config(chip, image.clone()))
            .await
            .unwrap();
        let mut text = Vec::new();
        let found = s
            .uart
            .wait_for(b"esp32bench-arduino tick 5", Duration::from_secs(30), |b| {
                text.extend_from_slice(b)
            })
            .await
            .unwrap();
        let text = String::from_utf8_lossy(&text);
        assert!(
            !text.contains("Guru Meditation"),
            "{chip} run {run} panicked:\n{text}"
        );
        assert_eq!(
            found,
            WaitOutcome::Found,
            "{chip} run {run}: no tick 5:\n{text}"
        );
        s.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn lcgamboa_esp32_runs_arduino() {
    arduino_keeps_running_on_lcgamboa(Chip::Esp32, 3).await;
}

#[tokio::test]
async fn lcgamboa_esp32c3_runs_arduino() {
    arduino_keeps_running_on_lcgamboa(Chip::Esp32c3, 1).await;
}

/// Wi-Fi firmware on the fork: it has a radio (without one the fork's Wi-Fi start-up
/// crashes on the ESP32 and the C3 aborts), and no network behind it (rule 4): it
/// starts, gets no address, and keeps running. The C3's radio needs an eFuse image.
#[tokio::test]
async fn lcgamboa_esp32_wifi_has_a_radio_but_no_network() {
    wifi_has_a_radio_but_no_network(Chip::Esp32).await;
}

#[tokio::test]
async fn lcgamboa_esp32c3_wifi_has_a_radio_but_no_network() {
    wifi_has_a_radio_but_no_network(Chip::Esp32c3).await;
}

async fn wifi_has_a_radio_but_no_network(chip: Chip) {
    let Some(image) = built_image("wifi-sta", chip) else {
        eprintln!("skipping: wifi-sta not built");
        return;
    };
    let Ok(launch) = esp32bench_sim::engine::launch(chip, Some(esp32bench_sim::Engine::Lcgamboa))
    else {
        eprintln!("skipping: lcgamboa's QEMU is not built");
        return;
    };
    let mut s = Session::start(launch.config(chip, image)).await.unwrap();
    let mut text = Vec::new();
    // Ten seconds: long enough for a crash, a reboot or a DHCP lease to show.
    let _ = s
        .uart
        .wait_for(b"never printed", Duration::from_secs(10), |b| {
            text.extend_from_slice(b)
        })
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&text);
    assert!(text.contains("esp32bench-wifi start"), "{text}");
    assert!(!text.contains("Guru Meditation"), "{text}");
    assert_eq!(text.matches("rst:").count(), 1, "rebooted:\n{text}");
    assert!(!text.contains("got ip"), "it reached a network:\n{text}");
    s.shutdown().await.unwrap();
}

/// Floating point in a global constructor runs (patch 0010: the S3 had the FPU off
/// from reset), and the ADC, LEDC and RMT calls the library's sketches make run
/// through on the fork:
/// analogRead() (on the S3, ADC calibration ran before setup() and hung until patch
/// 0004), tone() and analogWrite() (LEDC; aborted on the C3 and S3 until patches 0004
/// and 0005), and rmtWrite() and rgbLedWrite() (RMT, as WS2812 LEDs use it; never
/// finished until patch 0007).
async fn arduino_peripherals_on_lcgamboa(chip: Chip) {
    let Some(image) = built_image("arduino-periph", chip) else {
        eprintln!("skipping {chip}: arduino-periph not built");
        return;
    };
    let Ok(launch) = esp32bench_sim::engine::launch(chip, Some(esp32bench_sim::Engine::Lcgamboa))
    else {
        eprintln!("skipping {chip}: lcgamboa's QEMU is not built");
        return;
    };
    let mut s = Session::start(launch.config(chip, image)).await.unwrap();
    let mut text = Vec::new();
    let found = s
        .uart
        .wait_for(b"esp32bench-periph done", Duration::from_secs(30), |b| {
            text.extend_from_slice(b)
        })
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&text);
    assert_eq!(found, WaitOutcome::Found, "{chip}:\n{text}");
    for marker in [
        "esp32bench-periph fpu=6",
        "esp32bench-periph adc=",
        "esp32bench-periph tone",
        "esp32bench-periph pwm",
        "esp32bench-periph rmt=1",
        "esp32bench-periph rgb",
    ] {
        assert!(text.contains(marker), "{chip}: no {marker}:\n{text}");
    }
    assert!(!text.contains("abort()"), "{chip}:\n{text}");
    s.shutdown().await.unwrap();
}

#[tokio::test]
async fn lcgamboa_esp32_runs_adc_ledc_and_rmt() {
    arduino_peripherals_on_lcgamboa(Chip::Esp32).await;
}

#[tokio::test]
async fn lcgamboa_esp32s3_runs_adc_ledc_and_rmt() {
    arduino_peripherals_on_lcgamboa(Chip::Esp32s3).await;
}

#[tokio::test]
async fn lcgamboa_esp32c3_runs_adc_ledc_and_rmt() {
    arduino_peripherals_on_lcgamboa(Chip::Esp32c3).await;
}

/// Sleep on a timer, as battery sketches use it (patch 0008): light sleep returns,
/// and deep sleep restarts the chip through the ROM with reset reason DEEPSLEEP_RESET,
/// the wake-up cause "timer" and RTC memory kept (the sketch's boot count goes up).
/// Before the patch, light sleep never returned and deep sleep ended in a watchdog
/// reset with RTC memory lost.
async fn arduino_sleep_on_lcgamboa(chip: Chip) {
    let Some(image) = built_image("arduino-sleep", chip) else {
        return;
    };
    let Ok(launch) = esp32bench_sim::engine::launch(chip, Some(esp32bench_sim::Engine::Lcgamboa))
    else {
        eprintln!("skipping {chip}: lcgamboa's QEMU is not built");
        return;
    };
    let mut s = Session::start(launch.config(chip, image)).await.unwrap();
    let mut text = Vec::new();
    let found = s
        .uart
        .wait_for(b"esp32bench-sleep done", Duration::from_secs(30), |b| {
            text.extend_from_slice(b)
        })
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&text);
    assert_eq!(found, WaitOutcome::Found, "{chip}:\n{text}");
    for marker in [
        "esp32bench-sleep boot=1 cause=0 reset=1",
        "esp32bench-sleep light=",
        // ESP_SLEEP_WAKEUP_TIMER, ESP_RST_DEEPSLEEP
        "esp32bench-sleep boot=2 cause=4 reset=8",
        "esp32bench-sleep boot=3 cause=4 reset=8",
    ] {
        assert!(text.contains(marker), "{chip}: no {marker}:\n{text}");
    }
    assert!(!text.contains("Guru Meditation"), "{chip}:\n{text}");
    assert!(!text.contains("WDT"), "{chip}:\n{text}");
    s.shutdown().await.unwrap();
}

#[tokio::test]
async fn lcgamboa_esp32_sleeps_and_wakes() {
    arduino_sleep_on_lcgamboa(Chip::Esp32).await;
}

#[tokio::test]
async fn lcgamboa_esp32s3_sleeps_and_wakes() {
    arduino_sleep_on_lcgamboa(Chip::Esp32s3).await;
}

#[tokio::test]
async fn lcgamboa_esp32c3_sleeps_and_wakes() {
    arduino_sleep_on_lcgamboa(Chip::Esp32c3).await;
}

/// Light sleep as ESP-IDF 6.0.2 does it by default (patch 0013): three 500 ms sleeps
/// in a row each return, woken by the timer, with no watchdog. Before the patch the
/// interrupt watchdog fired during the second sleep on the ESP32 and C3: the timer
/// groups kept counting while the chip slept.
async fn idf_light_sleep_on_lcgamboa(chip: Chip) {
    let Some(image) = built_image("light-sleep", chip) else {
        return;
    };
    let Ok(launch) = esp32bench_sim::engine::launch(chip, Some(esp32bench_sim::Engine::Lcgamboa))
    else {
        eprintln!("skipping {chip}: lcgamboa's QEMU is not built");
        return;
    };
    let mut s = Session::start(launch.config(chip, image)).await.unwrap();
    let mut text = Vec::new();
    let found = s
        .uart
        .wait_for(
            b"esp32bench-light-sleep done",
            Duration::from_secs(30),
            |b| text.extend_from_slice(b),
        )
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&text);
    assert_eq!(found, WaitOutcome::Found, "{chip}:\n{text}");
    let slept: Vec<i64> = text
        .lines()
        .filter_map(|l| l.trim_end().strip_prefix("esp32bench-light-sleep slept="))
        .map(|l| {
            assert!(l.ends_with(" cause=4"), "{chip}: woken by the timer: {l}");
            l.split(' ').next().unwrap().parse().unwrap()
        })
        .collect();
    assert_eq!(slept.len(), 3, "{chip}:\n{text}");
    // ESP-IDF shortens the first sleep by the wake-up time it expects of the chip,
    // which the simulator does not take, then adapts; allow 10 ms.
    for us in slept {
        assert!(
            (490_000..=510_000).contains(&us),
            "{chip}: slept {us} us:\n{text}"
        );
    }
    assert!(!text.contains("Guru Meditation"), "{chip}:\n{text}");
    assert!(!text.contains("WDT"), "{chip}:\n{text}");
    s.shutdown().await.unwrap();
}

#[tokio::test]
async fn lcgamboa_esp32_idf_light_sleep() {
    idf_light_sleep_on_lcgamboa(Chip::Esp32).await;
}

#[tokio::test]
async fn lcgamboa_esp32s3_idf_light_sleep() {
    idf_light_sleep_on_lcgamboa(Chip::Esp32s3).await;
}

#[tokio::test]
async fn lcgamboa_esp32c3_idf_light_sleep() {
    idf_light_sleep_on_lcgamboa(Chip::Esp32c3).await;
}

/// The simulated Wi-Fi network (`esp32bench_sim::network`): the session joins
/// `tests/firmware/arduino-wifi` through its settings block, as the app does for
/// library sketches; the sketch gets an address by DHCP, cannot resolve names or
/// connect anywhere (out to the internet or to the host), and its web server answers
/// on the session's forwarded port.
async fn simulated_wifi_on_lcgamboa(chip: Chip) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let Some(image) = built_image("arduino-wifi", chip) else {
        return;
    };
    let Ok(launch) = esp32bench_sim::engine::launch(chip, Some(esp32bench_sim::Engine::Lcgamboa))
    else {
        eprintln!("skipping {chip}: lcgamboa's QEMU is not built");
        return;
    };
    let mut cfg = launch.config(chip, image);
    cfg.network = Some(esp32bench_sim::network::Network::on_a_free_port().unwrap());
    let mut s = Session::start(cfg).await.unwrap();
    let web = s.network().unwrap().web_port;
    let mut joiner = esp32bench_sim::network::Joiner::new();
    let mut text = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(40);
    while !String::from_utf8_lossy(&text).contains("esp32bench-wifi serving") {
        let chunk = tokio::time::timeout_at(deadline, s.uart.read_chunk())
            .await
            .unwrap_or_else(|_| panic!("{chip}: not serving:\n{}", String::from_utf8_lossy(&text)))
            .unwrap()
            .unwrap();
        if let Some(lines) = joiner.feed(&chunk) {
            s.uart.write(&lines).await.unwrap();
        }
        text.extend_from_slice(&chunk);
    }
    let text = String::from_utf8_lossy(&text);
    assert!(
        joiner.done(),
        "{chip}: never asked for its settings:\n{text}"
    );
    for marker in [
        "esp32bench-wifi ip=10.0.2.15",
        "esp32bench-wifi dns=0",
        "esp32bench-wifi out=0",
        "esp32bench-wifi host=0",
    ] {
        assert!(text.contains(marker), "{chip}: no {marker}:\n{text}");
    }
    let mut tcp = tokio::net::TcpStream::connect(("127.0.0.1", web))
        .await
        .unwrap();
    tcp.write_all(b"GET / HTTP/1.0\r\nHost: x\r\n\r\n")
        .await
        .unwrap();
    let mut reply = String::new();
    tokio::time::timeout(Duration::from_secs(10), tcp.read_to_string(&mut reply))
        .await
        .unwrap()
        .unwrap();
    assert!(reply.contains("esp32bench-web ok"), "{chip}: {reply}");
    s.shutdown().await.unwrap();
}

#[tokio::test]
async fn lcgamboa_esp32_simulated_wifi_is_isolated_and_serves() {
    simulated_wifi_on_lcgamboa(Chip::Esp32).await;
}

#[tokio::test]
async fn lcgamboa_esp32c3_simulated_wifi_is_isolated_and_serves() {
    simulated_wifi_on_lcgamboa(Chip::Esp32c3).await;
}

#[tokio::test]
async fn lcgamboa_esp32s3_simulated_wifi_is_isolated_and_serves() {
    simulated_wifi_on_lcgamboa(Chip::Esp32s3).await;
}

/// With `icount` (QEMU's -icount: time counted in instructions) an Arduino sketch
/// still boots and keeps time: its once-a-second tick comes. On the library build this
/// needed patch 0009 (an -O0 library and a 100 ms eFuse read made boot look hung).
async fn icount_on_lcgamboa(chip: Chip) {
    let Some(image) = built_image("hello-arduino", chip) else {
        return;
    };
    let Ok(launch) = esp32bench_sim::engine::launch(chip, Some(esp32bench_sim::Engine::Lcgamboa))
    else {
        eprintln!("skipping {chip}: lcgamboa's QEMU is not built");
        return;
    };
    let mut cfg = launch.config(chip, image);
    cfg.icount = true;
    let mut s = Session::start(cfg).await.unwrap();
    let mut text = Vec::new();
    let found = s
        .uart
        .wait_for(b"esp32bench-arduino tick 1", Duration::from_secs(30), |b| {
            text.extend_from_slice(b)
        })
        .await
        .unwrap();
    assert_eq!(
        found,
        WaitOutcome::Found,
        "{chip}:\n{}",
        String::from_utf8_lossy(&text)
    );
    s.shutdown().await.unwrap();
}

#[tokio::test]
async fn lcgamboa_esp32_runs_with_icount() {
    icount_on_lcgamboa(Chip::Esp32).await;
}

#[tokio::test]
async fn lcgamboa_esp32s3_runs_with_icount() {
    icount_on_lcgamboa(Chip::Esp32s3).await;
}

#[tokio::test]
async fn lcgamboa_esp32c3_runs_with_icount() {
    icount_on_lcgamboa(Chip::Esp32c3).await;
}

/// Boards without a USB-UART chip, like the C3 SuperMini, put `Serial` on the C3's
/// USB-Serial-JTAG. On the fork (patch 0006) that is part of the console: the sketch's
/// output arrives, and a line sent comes back through `Serial`.
#[tokio::test]
async fn lcgamboa_esp32c3_usb_console_both_ways() {
    let chip = Chip::Esp32c3;
    let image = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../tests/firmware/out/hello-arduino-esp32c3-usb.bin");
    if !image.is_file() {
        eprintln!("skipping: hello-arduino-esp32c3-usb not built");
        return;
    }
    let Ok(launch) = esp32bench_sim::engine::launch(chip, Some(esp32bench_sim::Engine::Lcgamboa))
    else {
        eprintln!("skipping: lcgamboa's QEMU is not built");
        return;
    };
    let mut cfg = launch.config(chip, image);
    cfg.usb_console = true;
    let mut s = Session::start(cfg).await.unwrap();
    let mut text = Vec::new();
    let found = s
        .uart
        .wait_for(b"esp32bench-arduino ready", Duration::from_secs(30), |b| {
            text.extend_from_slice(b)
        })
        .await
        .unwrap();
    assert_eq!(
        found,
        WaitOutcome::Found,
        "{}",
        String::from_utf8_lossy(&text)
    );
    s.uart.write(b"over usb\n").await.unwrap();
    let found = s
        .uart
        .wait_for(
            b"esp32bench-arduino echo over usb",
            Duration::from_secs(10),
            |b| text.extend_from_slice(b),
        )
        .await
        .unwrap();
    assert_eq!(
        found,
        WaitOutcome::Found,
        "{}",
        String::from_utf8_lossy(&text)
    );
    s.shutdown().await.unwrap();
}
