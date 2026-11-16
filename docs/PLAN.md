# Plan: Arch-native ESP32 workbench and simulator

Last updated: 2026-11-15

## Summary

Build a native Qt 6 desktop app for KDE Plasma, backed by a Rust core that wraps Espressif's QEMU fork for simulation and uses Rust serial and flash crates for real boards. Ship it as a PKGBUILD.

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
| Licence | GPL (decided) | Allows linking lcgamboa's QEMU library and reusing PICSimLab code. Pick GPLv2-or-later or GPLv3 once the phase 0 fork choice is made. |
| Build systems | `arduino-cli` and `idf.py` from the start | The site's firmware is written for the Arduino IDE, so Arduino builds cannot wait. PlatformIO comes later. |
| Simulation engine | Espressif's QEMU fork or lcgamboa's fork of it, chosen in phase 0 | Both are maintained. Writing your own Xtensa/RISC-V core is not realistic. |
| Getting QEMU | Espressif's prebuilt binaries, or a library built from lcgamboa's fork | The AUR package is stale (see Arch-specific notes). |
| Toolchain | Detect an existing ESP-IDF and Arduino core, offer a managed install | The AUR `esp-idf` package is one supported source. |

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

The simulator is firmware-level, not circuit-level. Details below come from the [ESP-IDF QEMU guide](https://docs.espressif.com/projects/esp-idf/en/v6.0.2/esp32/api-guides/tools/qemu.html).

- **Scope:** the fork covers the CPU, memory and several peripherals, not the whole chip. Documented targets include ESP32, ESP32-S3 and ESP32-C3.
- **Install:** prebuilt x86_64 Linux binaries come through `idf_tools.py install qemu-xtensa qemu-riscv32`. Arch runtime dependencies are `libgcrypt glib2 pixman sdl2 libslirp`, all in the official repos.
- **Debugging:** `idf.py qemu gdb` gives a GDB session against the emulated chip, so the app can offer breakpoints and register views without hardware.
- **Display:** a virtual framebuffer exists for testing graphics apps. It has no counterpart on real ESP32 hardware.
- **Security testing:** eFuse emulation lets you try secure boot and flash encryption without irreversible burns.
- **Gaps:** no Wi-Fi radio emulation in Espressif's fork, confirmed in phase 0: firmware that starts Wi-Fi panics on ESP32 and hangs on S3 and C3. BLE, and GPIO, ADC and I2C modelling, are still unverified. Results are in [qemu-forks.md](qemu-forks.md).

## Architecture

A Rust core process does the work and a Qt 6 UI shows it, talking over a local socket. A project runs against either QEMU or a physical port through the same monitor and debug UI.

1. **Rust core (daemon + CLI)**
    - Process supervisor for QEMU, controlled over QMP, with UART on a socket chardev.
    - Serial layer for real boards (`serialport`), flashing via `espflash` as a library, and device hotplug via udev.
    - Build runner that shells out to `idf.py` and `arduino-cli`, with PlatformIO as a later option.
    - GDB/MI client for both QEMU and real boards (USB-Serial-JTAG or OpenOCD).
    - JSON-RPC API over a Unix socket, with event streams for serial output, build logs and device hotplug.
2. **UI (Qt 6 Widgets, C++)**
    - Dockable panels: project library, board picker, serial monitor, flash and partition view, simulator panel and debug panel.
    - Serial monitor on a terminal widget (QTermWidget, if its Qt 6 build fits; otherwise a plain text view).
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

Phases 0 to 3 and 5 total about 33 weeks, roughly 8 months, at about 14 hours a week (11 weeks full-time). A usable workbench release lands after phase 2, about 21 weeks in. Phase 4 is open-ended.

| Phase | Deliverable | Rough effort at ~14 h/week |
| --- | --- | --- |
| 0. Spike | Run a hello-world in QEMU from a Rust CLI: launch, UART over socket, QMP reset, GDB attach. Run the same test against lcgamboa's fork with a current IDF build, write down each fork's peripheral gaps, and pick one. | 3 weeks |
| 1. Shell | Core daemon with its JSON-RPC API, and a Qt app with the project library imported from esp32-bench data, board detection, the serial monitor, arduino-cli builds and a first-run toolchain setup. | 12 weeks |
| 2. Real hardware | Flash, erase, partition table and eFuse summary. Ship a udev rule and handle the `uucp` group. | 6 weeks |
| 3. Simulator v1 | Build, flash image, run in QEMU, with console, reset, GDB panel and embedded framebuffer. | 9 weeks |
| 4. Virtual parts | LEDs, buttons and I2C sensors on a QGraphicsScene pin diagram. | 12+ weeks, open-ended |
| 5. Packaging | PKGBUILD (`-git` and `-bin`) building both the Rust core and the Qt UI, desktop entry, optional deps declared, AUR submission. | 3 weeks |

Phase 4 depends on the phase 0 choice. With lcgamboa's fork, GPIO, ADC and I2C hooks already exist and the work is mostly the canvas and part models. With Espressif's fork, it needs patches to expose GPIO state or custom QEMU device models, and should be scoped down to UART, display and I2C.

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
- **QEMU package:** don't depend on [qemu-esp-xtensa-git](https://aur.archlinux.org/packages/qemu-esp-xtensa-git). Its last update was March 2023, and an [orphan request](https://lists.archlinux.org/archives/list/aur-requests@lists.archlinux.org/message/5VD4BOWX7OM42THGVOPNNM3RFZ2SU77H) cites an issue unresolved since 2023. Fetch Espressif's binaries into `~/.local/share/<app>/`, or write your own `-bin` PKGBUILD.
- **Qt:** build the UI against `qt6-base` with CMake and the core with Cargo. Both toolchains are in the official repos, so the UI needs no AUR dependencies. Test on Plasma Wayland first, then X11.

## Risks

1. **Peripheral fidelity:** users will expect Wokwi-level parts. Label the simulator as firmware-level, not circuit-level.
2. **QEMU fork drift:** pin a known-good release per IDF version and test the pairing in CI.
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
