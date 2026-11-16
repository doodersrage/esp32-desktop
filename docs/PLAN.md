# Plan: Arch-native ESP32 workbench and simulator

Last updated: 2026-11-16 (phases 1 to 3 feature complete, awaiting checks on real hardware and Plasma)

## Summary

Build a native Qt 6 desktop app for KDE Plasma, backed by a Rust core that wraps lcgamboa's QEMU fork (Espressif's QEMU plus Wi-Fi and host hooks) for simulation and uses Rust serial and flash crates for real boards. Ship it as a PKGBUILD.

The one hard problem is the virtual breadboard: GPIO-level parts wired to the emulated chip. The plan defers it until the rest works.

## Scope for v1

v1 is done when every esp32bench.dev library project builds and flashes from the app, and every ESP32, S3 and C3 project boots in the simulator.

- **Simulated boards:** ESP32, ESP32-S3 and ESP32-C3 only, because that is what QEMU covers.
- **Workbench-only boards:** S2, C6, the XIAO boards and ESP32-CAM get build, flash, monitor and debug, with no simulation. The UI shows this per project.
- **Not in v1:** a built-in code editor (projects open in Kate or VS Code), analog circuit simulation, Windows or macOS builds, and Flatpak packaging.

## Key decisions

| Decision | Recommendation | Why |
| --- | --- | --- |
| GUI toolkit | Qt 6 Widgets, in C++ | Matches Plasma. QGraphicsScene is the strongest canvas for the virtual breadboard, and dock widgets suit an IDE-like layout. GTK4 + libadwaita would look out of place on Plasma; a Tauri webview was dropped in favour of a native UI. |
| UI to core link | Rust core as a separate process, JSON-RPC over a Unix socket | Keeps the two build systems apart, makes the core testable headless, and gives a CLI for free. cxx-qt (Rust inside the Qt process) is the alternative. |
| Licence | GPLv2-or-later (decided in phase 0) | The simulator loads lcgamboa's QEMU library, and QEMU is GPLv2, which rules out GPLv3. GPLv2-or-later also allows reusing PICSimLab code (GPLv2). |
| Build systems | `arduino-cli` and `idf.py` from the start | The site's firmware is written for the Arduino IDE, so Arduino builds cannot wait. PlatformIO comes later. |
| Simulation engine | lcgamboa's QEMU fork, pinned to a commit (decided in phase 0) | Both forks passed boot, UART, reset, GDB and the pinned Arduino core on all three chips; only lcgamboa's lets the host read and drive GPIO and answer as an I2C device, and it adds Wi-Fi on ESP32 and C3. That makes phase 4 app work rather than QEMU patches. Results: [PHASE0.md](PHASE0.md), [qemu-forks.md](qemu-forks.md). |
| Getting QEMU | Build lcgamboa's fork from source at the pinned commit, as executables and as a library | The fork has no releases. The AUR package is stale (see Arch-specific notes). Espressif's prebuilt binaries stay as a test baseline until the core's setup step builds the fork. |
| Toolchain | Detect an existing ESP-IDF and Arduino core, offer a managed install | The AUR `esp-idf` package is one supported source. |
| Serial monitor | Plain text view (`QPlainTextEdit`) (decided 2026-11-16) | Line-based output needs no terminal emulation, and `qt6-base` already has it. |

The cost of going native is that the esp32-bench Astro UI is not reused. The shared data schema still carries the library content over.

## Prior art

No existing tool combines a native Linux desktop app, an ESP32 focus, and both workbench and simulator. The closest projects each cover one part.

| Project | What it is | Where it falls short here |
| --- | --- | --- |
| [PICSimLab](https://github.com/lcgamboa/picsimlab/) | Open-source (GPLv2) desktop board emulator for many chip families, with a qemu-esp32 backend. | A multi-MCU teaching tool with no project library or real-board workflow, and not a Qt/Plasma app. |
| [Velxio](https://www.cnx-software.com/2026/04/04/velxio-open-source-self-hosted-arduino-raspberry-pi-and-esp32-simulator/) | Open-source Wokwi-style simulator; ESP32 and ESP32-S3 run through QEMU. Self-hosted in Docker. | Browser UI, not native. AGPLv3 unless you buy a commercial licence. |
| [SimulIDE](https://simulide.com/p/forum/topic/esp-32-support) | Qt circuit simulator, the nearest thing to a native virtual breadboard. | Its author calls the ESP32 support a proof of concept. |
| Wokwi | The most polished ESP32 simulator, in the browser and VS Code. | Freemium, and not a native app. |

The useful find is [lcgamboa's QEMU fork](https://github.com/lcgamboa/qemu), which PICSimLab and Velxio both use. It builds QEMU as a dynamic library, adds Wi-Fi, and exposes GPIO, ADC, UART and I2C to the host app through callbacks. That is most of the phase 4 problem already solved.

Two caveats: [Velxio's docs](https://git.manakin-gentoo.ts.net/a2nr/velxio/src/branch/master/docs/ESP32_EMULATION.md) pin the fork to QEMU 8.1.3 and IDF 4.4.x, so it may lag IDF 6.x. Linking it also brings GPL obligations.

## What the simulator can and can't do

The simulator is firmware-level, not circuit-level. It runs on lcgamboa's fork of Espressif's QEMU; phase 0 tested what works ([PHASE0.md](PHASE0.md), [qemu-forks.md](qemu-forks.md)).

- **Chips:** ESP32 (on the fork's `esp32-picsimlab` machine; its plain `esp32` machine aborts at start-up), ESP32-S3 and ESP32-C3 (`esp32c3-picsimlab`).
- **Firmware:** ESP-IDF v6.0.2 and ESP32 Arduino core 3.3.12 builds boot on all three. Older IDF 4.4 builds (Arduino core 2.x) fail flash init on S3 and C3.
- **Console, reset and debugging:** UART0 in and out over a socket, QMP reset, and GDB breakpoints and stepping work on all three chips.
- **Wi-Fi:** station and soft-AP on ESP32 and C3, against hard-coded open networks. Not on S3. Emulated networking gives guest firmware network access, so it needs consent; until that exists, every session runs with `-nic none`.
- **GPIO and I2C:** through the library build and a per-session host process, on ESP32 and C3 only. GPIO outputs are reported for GPIO 0–31 only. I2C answers are synchronous, so part models run inside the host process.
- **S3:** boots, console and GDB only. No Wi-Fi, GPIO or I2C hooks, so no virtual parts.
- **eFuses:** blank unless an eFuse image is supplied (the MAC reads as zeros). C3 Wi-Fi needs one.
- **Display:** QEMU's virtual RGB panel (firmware using Espressif's `esp_lcd_qemu_rgb`) shows in the app on all three chips, read through QMP `screendump`. Real display parts (SSD1306, ST7789...) are not modelled; that is phase 4 work.
- **Patches:** the fork is built with two esp32bench patches (`core/crates/sim/patches/`): pixman in the library build, and the "QEMU" signature register on `esp32-picsimlab`, both for the display.
- **Unverified:** BLE, ADC (`qemu_picsimlab_set_apin` exists), SPI and RMT hooks, and secure boot or flash encryption.
- **Arch dependencies:** `libgcrypt glib2 pixman libslirp`, all in the official repos.

## Architecture

A Rust core process does the work and a Qt 6 UI shows it, talking over a local socket. A project runs against either QEMU or a physical port through the same monitor and debug UI.

1. **Rust core (daemon + CLI)**
    - Process supervisor for QEMU, controlled over QMP, with UART on a socket chardev. For GPIO and I2C, each session runs a small host process that loads the fork's QEMU library and serves pin events on a socket (`esp32bench-qemu-host`, built in phase 0).
    - Serial layer for real boards (`serialport`), flashing via `espflash` as a library, and device hotplug via udev.
    - Build runner that shells out to `idf.py` and `arduino-cli`, with PlatformIO as a later option.
    - GDB/MI client for both QEMU and real boards (USB-Serial-JTAG or OpenOCD).
    - JSON-RPC API over a Unix socket, with event streams for serial output, build logs and device hotplug. Wire format, errors, events and versioning are in [rpc.md](rpc.md); methods are defined in `schema/rpc/`.
2. **UI (Qt 6 Widgets, C++)**
    - Dockable panels: project library, board picker, serial monitor, flash and partition view, simulator panel and debug panel.
    - Serial monitor on a plain text view (`QPlainTextEdit`), decided 2026-11-16: ESP32 output is line-based text, and it keeps the UI on `qt6-base` with no AUR dependencies. QTermWidget would only pay off for interactive terminal programs, which the library projects are not.
    - QGraphicsScene canvas for pin diagrams and, later, virtual parts.
    - QEMU framebuffer embedded as a widget instead of a separate SDL window.
3. **Shared data**
    - Board definitions, pinouts and project metadata in one JSON schema used by both the web app and the desktop app.
    - ProbeHarbor board data fits here.
4. **Simulation targets**
    - A "target" abstraction covering a QEMU machine or a physical port.

## Integration with esp32bench.dev

[esp32bench.dev](https://esp32bench.dev/) and the desktop app are one product with two front ends. The site is the zero-install entry point; the app does what a browser cannot: simulation, GDB, JTAG and local toolchains.

1. **Shared project library:** the Astro build emits a versioned JSON manifest of all projects (pin maps, parts lists, firmware). The app fetches it and caches it for offline use.
2. **"Open in desktop app" links:** each project page gets a button using a custom URL scheme such as `esp32bench://project/<slug>`, registered in the app's `.desktop` file. The app always asks before building or flashing anything opened from a link.
3. **Shared sync:** the app speaks the site's sync protocol (sync code or GitHub sign-in), so build log, shopping list and parts drawer match on both sides. Tokens are stored in KWallet.
4. **Simulation-verified projects:** CI runs the headless core to build and boot every library project in QEMU. Passing projects get a "runs in simulator" badge on the site.
5. **Crash translator:** the app decodes backtraces against the local ELF file and reuses the site's error explanations.

## Phases

Phases 0 to 3 and 5 total about 33 weeks, roughly 8 months, at about 14 hours a week (11 weeks full-time). A usable workbench release lands after phase 2, about 21 weeks in. Phase 4 is open-ended. Phase 0 took about 4 hours, not 3 weeks, because prebuilt QEMU and an existing fork did most of the work. Phases 1 to 3 reached feature complete in two days of sessions (2026-11-15 and 16) against 27 estimated weeks; phases 4 and 5 will be rescaled once those phases pass their checks on Plasma and hardware and are marked done.

| Phase | Deliverable | Rough effort at ~14 h/week |
| --- | --- | --- |
| 0. Spike (done 2026-11-15) | Run a hello-world in QEMU from a Rust CLI: launch, UART over socket, QMP reset, GDB attach. Run the same test against lcgamboa's fork with a current IDF build, write down each fork's peripheral gaps, and pick one. | 3 weeks; actual about 4 hours |
| 1. Shell (feature complete 2026-11-16; awaiting a Plasma check) | Core daemon with its JSON-RPC API, and a Qt app with the project library imported from esp32-bench data, board detection, the serial monitor, arduino-cli builds and a first-run toolchain setup. | 12 weeks |
| 2. Real hardware (feature complete 2026-11-16; board operations checked on an ESP32-C3, erase and UART-bridge boards not yet) | Flash, erase, partition table and eFuse summary. Ship a udev rule and handle the `uucp` group. | 6 weeks |
| 3. Simulator v1 (feature complete 2026-11-16; awaiting a Plasma check) | Build and pin lcgamboa's fork in the setup step (executables and library). Build, flash image, run in QEMU, with console, reset, GDB panel and embedded framebuffer. | 9 weeks |
| 4. Virtual parts | LEDs, buttons and I2C sensors on a QGraphicsScene pin diagram, for ESP32 and C3, driven through the fork's library hooks. | 12+ weeks, open-ended |
| 5. Packaging | PKGBUILD (`-git` and `-bin`) building both the Rust core and the Qt UI, desktop entry, optional deps declared, AUR submission. | 3 weeks |

Phase 4 builds on lcgamboa's library hooks, which phase 0 proved for GPIO and I2C. The work is mostly the canvas and the part models:

- **Host process:** grow `esp32bench-qemu-host` from the phase 0 spike into the per-session bridge. It fixes the bridged pins at start-up (the fork reads the pin map once) and streams pin events to the core, which forwards them to the UI.
- **Part models run in the host:** the I2C callback must answer synchronously, so sensors and other bus devices live in the host process, loaded from the part model files. The host answers for a whole bus, so it models every device on it.
- **Limits to design around:** GPIO outputs only for GPIO 0–31; no hooks on S3, so S3 projects get no virtual parts; ADC, SPI and RMT hooks exist but are untested.

## Testing

- **Core:** unit tests plus QEMU boot tests in CI for each supported chip and each pinned IDF and Arduino core version.
- **Library:** the same CI job that feeds the site's "runs in simulator" badge doubles as a regression suite.
- **Hardware in the loop:** one physical board per chip family on a USB hub, running flash, monitor and reset tests before each release.
- **UI:** smoke tests on Plasma Wayland and X11.

## Also plan for

- **First-run setup:** toolchains are several gigabytes. A setup wizard detects what is installed, shows download sizes, and supports several IDF and Arduino core versions side by side.
- **Translations:** the site ships in five languages. Wrap every UI string for Qt translation from the first view.
- **Signal trace view:** with GPIO hooks, a logic-analyzer panel for simulated pins is cheap to add. Export VCD so traces open in PulseView.
- **Part model format:** define virtual parts (LED, button, sensor) as data files plus a small script, so new parts do not need a C++ change.
- **Project format:** a plain, git-friendly on-disk layout for local projects, compatible with the site's manifest.
- **Repo layout:** a separate repo from the site, with the shared schema published as its own package.

## Arch-specific notes

- **Toolchain:** the AUR [esp-idf](https://aur.archlinux.org/packages/esp-idf/) package is at 6.0.2-1 and installs under `/opt/esp-idf`. Detect that path and `~/esp`, and still support a per-project IDF version.
- **QEMU package:** don't depend on [qemu-esp-xtensa-git](https://aur.archlinux.org/packages/qemu-esp-xtensa-git). Its last update was March 2023, and an [orphan request](https://lists.archlinux.org/archives/list/aur-requests@lists.archlinux.org/message/5VD4BOWX7OM42THGVOPNNM3RFZ2SU77H) cites an issue unresolved since 2023. The chosen fork has no releases: build it at the pinned commit into `~/.local/share/esp32bench/`, or ship it as our own `-bin` PKGBUILD.
- **Qt:** build the UI against `qt6-base` with CMake and the core with Cargo. Both toolchains are in the official repos, so the UI needs no AUR dependencies. Test on Plasma Wayland first, then X11.

## Risks

1. **Peripheral fidelity:** users will expect Wokwi-level parts. Label the simulator as firmware-level, not circuit-level.
2. **QEMU fork drift:** lcgamboa's fork has no releases and moves often. Pin a known-good commit per IDF version and test the pairing in CI. Its plain `esp32` machine was already broken at the pinned commit.
3. **Scope:** workbench and simulator are two products. Phases 1 and 2 are useful on their own, so ship them first.

## Sources

- [ESP-IDF QEMU Emulator guide](https://docs.espressif.com/projects/esp-idf/en/v6.0.2/esp32/api-guides/tools/qemu.html)
- [AUR: esp-idf](https://aur.archlinux.org/packages/esp-idf/)
- [AUR: qemu-esp-xtensa-git](https://aur.archlinux.org/packages/qemu-esp-xtensa-git)
- [AUR orphan request for qemu-esp-xtensa-git](https://lists.archlinux.org/archives/list/aur-requests@lists.archlinux.org/message/5VD4BOWX7OM42THGVOPNNM3RFZ2SU77H)
- [PICSimLab](https://github.com/lcgamboa/picsimlab/)
- [lcgamboa's QEMU fork](https://github.com/lcgamboa/qemu)
- [Velxio ESP32 emulation docs](https://git.manakin-gentoo.ts.net/a2nr/velxio/src/branch/master/docs/ESP32_EMULATION.md)
- [SimulIDE forum: ESP32 support](https://simulide.com/p/forum/topic/esp-32-support)
- [esp32bench.dev](https://esp32bench.dev/)
