# Plan: native Linux ESP32 workbench and simulator

Last updated: 2026-11-21 (0.1.9 released: readable, filterable logs, real boards checked against their project, many usability changes, and a fix for console output the simulator lost under load (fork patch 0017, recipe 15); 0.1.8 released: a toolbar and many usability changes, prebuilt firmware, output checks and explanations, a plotter; 0.1.7 released, the first with macOS and Windows packages; phase 6: macOS and Windows packages build and pass their smoke tests in CI, the simulator shipped prebuilt there; the owner made Windows and macOS target platforms on 2026-11-19; 0.1.6 released: the UI in five languages, a signal trace, and the S3 with pins, parts and analog inputs on fork recipe 14; 0.1.5 released, the first with `.deb` and `.rpm` packages: the owner made Debian, Ubuntu and Fedora supported platforms beside Arch, with packages built and install-tested in CI; 0.1.4 released: the user's own ESP-IDF projects build with idf.py; the simulator section reflects fork recipe 13)

## Summary

Build a native Qt 6 desktop app for Linux (developed on KDE Plasma; Arch, Debian, Ubuntu and Fedora supported since 2026-11-19), with macOS and then Windows to follow (phase 6, added 2026-11-19), backed by a Rust core that wraps lcgamboa's QEMU fork (Espressif's QEMU plus Wi-Fi and host hooks) for simulation and uses Rust serial and flash crates for real boards. Ship it as a PKGBUILD.

The one hard problem is the virtual breadboard: GPIO-level parts wired to the emulated chip. The plan defers it until the rest works.

## Scope for v1

v1 is done when every esp32bench.dev library project builds and flashes from the app, and every ESP32, S3 and C3 project boots in the simulator.

- **Simulated boards:** ESP32, ESP32-S3 and ESP32-C3 only, because that is what QEMU covers.
- **Workbench-only boards:** S2, C6, the XIAO boards and ESP32-CAM get build, flash, monitor and debug, with no simulation. The UI shows this per project.
- **Platforms:** Linux first. macOS and Windows were added by the owner on 2026-11-19, because most ESP32 hobbyists use them; they are supported once phase 6 is done, macOS first.
- **Not in v1:** a built-in code editor (projects open in Kate or VS Code), analog circuit simulation, and Flatpak packaging.

## Key decisions

| Decision | Recommendation | Why |
| --- | --- | --- |
| GUI toolkit | Qt 6 Widgets, in C++ | Matches Plasma. QGraphicsScene is the strongest canvas for the virtual breadboard, and dock widgets suit an IDE-like layout. GTK4 + libadwaita would look out of place on Plasma; a Tauri webview was dropped in favour of a native UI. |
| UI to core link | Rust core as a separate process, JSON-RPC over a Unix socket | Keeps the two build systems apart, makes the core testable headless, and gives a CLI for free. cxx-qt (Rust inside the Qt process) is the alternative. |
| Licence | GPLv2-or-later (decided in phase 0) | The simulator loads lcgamboa's QEMU library, and QEMU is GPLv2, which rules out GPLv3. GPLv2-or-later also allows reusing PICSimLab code (GPLv2). |
| Build systems | `arduino-cli` and `idf.py` from the start | The site's firmware is written for the Arduino IDE, so Arduino builds cannot wait. PlatformIO comes later. |
| Simulation engine | lcgamboa's QEMU fork, pinned to a commit (decided in phase 0) | Both forks passed boot, UART, reset, GDB and the pinned Arduino core on all three chips; only lcgamboa's lets the host read and drive GPIO and answer as an I2C device, and it adds Wi-Fi on ESP32 and C3. That makes phase 4 app work rather than QEMU patches. Results: [PHASE0.md](PHASE0.md), [qemu-forks.md](qemu-forks.md). |
| Getting QEMU | On Linux, build lcgamboa's fork from source at the pinned commit, as executables and as a library. On macOS and Windows the release ships the patched fork's library prebuilt (decided 2026-11-20 from phase 6's spike, [platforms.md](platforms.md): a user build there needs Homebrew or MSYS2, and on Windows a 30-minute static build) | The fork has no releases. The AUR package is stale (see Arch-specific notes). Espressif's prebuilt binaries stay as a test baseline until the core's setup step builds the fork. |
| Toolchain | Detect an existing ESP-IDF and Arduino core, offer a managed install | The AUR `esp-idf` package is one supported source. |
| Platforms | Arch, Debian 13, Ubuntu 24.04 and 26.04 LTS, Fedora 44 and 45, on any desktop (decided by the owner 2026-11-19; before, Arch with Plasma only); macOS and Windows targeted since 2026-11-19 and released since 0.1.7 (2026-11-20; real boards there unchecked, phase 6) | Packages for each (`.pkg.tar.zst`, `.deb`, `.rpm`) are built in that release's container against its own Qt (6.4 at least, for Ubuntu 24.04) and libraries, installed into a fresh container and smoke-tested, by `.github/workflows/packages.yml` on every release tag. Where a distribution has no current `arduino-cli` package (Debian, Ubuntu, Fedora) the app installs a pinned, checksummed release itself; hints name each distribution's packages and serial group (`esp32bench-distro`). Hardware has been checked on Arch only. |
| Serial monitor | Plain text view (`QPlainTextEdit`) (decided 2026-11-16) | Line-based output needs no terminal emulation, and `qt6-base` already has it. |
| Simulator network | A simulated Wi-Fi network that reaches nothing: restricted user-mode networking, only the firmware's port 80 forwarded to loopback (decided 2026-11-18) | Library sketches that wait for Wi-Fi run and their web pages open on the desktop, while downloaded firmware still reaches no other computer and resolves no names, so no consent prompt is needed. |
| Virtual part data | Placed in the app first, saved per project and board on this computer; the shared manifest may carry the same data later (decided 2026-11-16) | The library's wiring is free text, so parts need structured data from somewhere. Placing them in the app works for every project at once with no site change. The circuit file format is versioned in `schema/` and designed so the manifest can adopt it as an optional addition. |
| Part models | A JSON description plus a [Rhai](https://rhai.rs) script per model, run in the per-session host process (decided 2026-11-16) | The plan asked for data files plus a small script. Rhai is pure Rust (no C library, no extra Arch package), sandboxed by default (no files, network or processes), and MIT-licensed. The host must answer I2C synchronously, so every part runs there, beside the QEMU library. Details: [parts.md](parts.md). |

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

- **Chips:** ESP32 (on the fork's `esp32-picsimlab` machine; its plain `esp32` machine aborts at start-up), ESP32-S3 (`esp32s3`, with the PICSimLab hooks since patch 0015) and ESP32-C3 (`esp32c3-picsimlab`).
- **Firmware:** ESP-IDF v6.0.2 and ESP32 Arduino core 3.3.12 builds boot on all three. Older IDF 4.4 builds (Arduino core 2.x) fail flash init on S3 and C3.
- **Console, reset and debugging:** UART0 in and out over a socket, QMP reset, and GDB breakpoints and stepping work on all three chips.
- **Wi-Fi:** station and soft-AP on ESP32, C3 and (since patch 0012, with the C3's radio models) S3, against the fork's one open access point (`PICSimLabWifi`). Decided 2026-11-18 (the owner): sessions get a simulated network that reaches nothing. QEMU's user-mode network in restricted mode sits on the radio's hub, so the firmware gets an address by DHCP and nothing else: no name lookups (checked: restricted mode drops them), no connections out or to the host (`tests/firmware/arduino-wifi`). The firmware's port 80 is forwarded to a free port on loopback so its web page opens on the desktop. Library sketches take their Wi-Fi settings over serial (the esp32bench.dev settings block), and the core answers with the simulated network's name and an empty password; WPA2 is not modelled (the fork's radio has no hardware crypto), and Arduino accepts an open network when the password is empty. `sim.start` can turn either off (`network`, `joinWifi`). Without a radio the fork's Wi-Fi start-up crashes, so sessions without the network keep the radio on an empty hub port; the C3 needs an eFuse image for its radio.
- **GPIO, I2C and RMT frames:** through the library build and a per-session host process, on all three chips (the S3 since patch 0015, 2026-11-19: before, it ran on the fork's executable with no hooks). GPIO outputs are reported for GPIO 0–31 only; the S3's GPIO 40–48 are not modelled. I2C answers are synchronous, so part models run inside the host process.
- **eFuses:** blank unless an eFuse image is supplied (the MAC reads as zeros). C3 Wi-Fi needs one; the ESP32's carries the ADC reference voltage and the S3's the ADC calibration version, without which ESP-IDF's ADC calibration fails.
- **Display:** QEMU's virtual RGB panel (firmware using Espressif's `esp_lcd_qemu_rgb`) shows in the app on all three chips, read through QMP `screendump`. An SSD1306 OLED is a virtual part (phase 4); other displays (ST7789...) are not modelled.
- **Patches:** the fork is built with sixteen esp32bench patches (`core/crates/sim/patches/`, listed in `CLAUDE.md` and explained in [qemu-forks.md](qemu-forks.md)): the display, the CPU clock, ADC and LEDC registers, the C3's USB serial console, RMT (WS2812), sleep, an optimised library, the FPU at reset, RMT frames for parts, the S3's radio, ESP-IDF light sleep and the ESP32's cache, the ESP32's eFuse timing, the S3's host hooks, and the S3's RTC slow clock.
- **Peripherals:** on all three chips, an analog input reads the voltage a part puts on its pin (a potentiometer part, set with a slider; added 2026-11-19), turned into the 12-bit reading that ESP-IDF's calibration maps back to that voltage. This is a value source like the sensors' sliders, not circuit simulation, which stays out of v1. Pins with no part read 0; LEDC and RMT run (WS2812 strips are parts); light and deep sleep wake on a timer (ESP-IDF builds' light sleep since patch 0013, 2026-11-19; `docs/idf-examples.md`). Not modelled: Bluetooth (neither fork has a controller), SPI parts, and secure boot or flash encryption.
- **Arch dependencies:** `libgcrypt glib2 pixman libslirp`, all in the official repos.

## Architecture

A Rust core process does the work and a Qt 6 UI shows it, talking over a local socket. A project runs against either QEMU or a physical port through the same monitor and debug UI.

1. **Rust core (daemon + CLI)**
    - Process supervisor for QEMU, controlled over QMP, with UART on a socket chardev. For GPIO and I2C, each session runs a small host process that loads the fork's QEMU library and serves pin events on a socket (`esp32bench-qemu-host`, built in phase 0).
    - Serial layer for real boards (`serialport`), flashing via `espflash` as a library, and device hotplug via udev.
    - Build runner that shells out to `arduino-cli` (the library's sketches) and `idf.py` (the user's own ESP-IDF project folders, added 2026-11-18 at the owner's request), with PlatformIO as a later option.
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

The first estimate was about 33 weeks for phases 0 to 3 and 5, at about 14 hours a week, with phase 4 open-ended. Actual effort has been far below it: phase 0 took about 4 hours, not 3 weeks, because prebuilt QEMU and an existing fork did most of the work, and phases 1 to 3 reached feature complete in two days of sessions (2026-11-15 and 16) against 27 estimated weeks. Phases 1 and 3 passed the owner's check on Plasma the same day (`docs/hardware-notes.md`); phase 2 waits only on the automatic DTR/RTS reset of a board with an auto-reset circuit (erase, and reading and writing through a UART bridge, were checked on the C3 on 2026-11-17). On that evidence phases 4 and 5 were rescaled on 2026-11-16: the remaining work is about 2 weeks at 14 hours a week, most of it checks on Plasma and real hardware and the AUR submission, which only the owner can do. Phase 4 then took two days of sessions and passed its Plasma check on 2026-11-17. Further part models stay open-ended. Phase 6 (macOS and Windows, added 2026-11-19) is estimated at 6 to 9 weeks, starting with a 1 to 2 day spike that decides how the simulator is delivered there; these estimates are rough until the spike is done.

| Phase | Deliverable | Rough effort at ~14 h/week |
| --- | --- | --- |
| 0. Spike (done 2026-11-15) | Run a hello-world in QEMU from a Rust CLI: launch, UART over socket, QMP reset, GDB attach. Run the same test against lcgamboa's fork with a current IDF build, write down each fork's peripheral gaps, and pick one. | 3 weeks; actual about 4 hours |
| 1. Shell (done 2026-11-16, checked on Plasma) | Core daemon with its JSON-RPC API, and a Qt app with the project library imported from esp32-bench data, board detection, the serial monitor, arduino-cli builds and a first-run toolchain setup. | 12 weeks; actual part of two days, with phases 2 and 3 |
| 2. Real hardware (feature complete 2026-11-16; inspect, flash, read, erase and restore checked on an ESP32-C3, over USB and a PL2303 UART bridge; the automatic DTR/RTS reset not yet) | Flash, erase, partition table and eFuse summary. Ship a udev rule and handle the `uucp` group. | 6 weeks |
| 3. Simulator v1 (done 2026-11-16, checked on Plasma) | Build and pin lcgamboa's fork in the setup step (executables and library). Build, flash image, run in QEMU, with console, reset, GDB panel and embedded framebuffer. | 9 weeks |
| 4. Virtual parts (done 2026-11-17, checked on Plasma; the S3 too since 2026-11-19) | LEDs, buttons and I2C sensors on a QGraphicsScene pin diagram, for ESP32, S3 and C3, driven through the fork's library hooks. In place: LED, button, switch, RGB LED, SSD1306 OLED, BME280, MPU-6050 and DS3231, with sensor controls kept between runs. | was 12+ weeks; actual two days of sessions (2026-11-16 and 17); more parts open-ended |
| 5. Packaging (in progress: 0.1.0 released 2026-11-17, 0.1.1 to 0.1.3 on 2026-11-18, 0.1.4 to 0.1.6 on 2026-11-19, with `-git` and `-bin` PKGBUILDs, clean-chroot built, namcap-checked and installed on Plasma; from 2026-11-19 also `.deb` and `.rpm` packages for Debian 13, Ubuntu 24.04 and 26.04 and Fedora 44 and 45, built and install-tested in CI per release; the AUR submission is left) | PKGBUILD (`-git` and `-bin`) building both the Rust core and the Qt UI, desktop entry, optional deps declared, AUR submission; Debian and Fedora packages from the same install layout (`packaging/install-files.sh`). | was 3 weeks; rescaled to about 1 week, including a clean-chroot build and the owner's AUR submission |
| 6. macOS and Windows (added 2026-11-19 by the owner; in progress: the spike done 2026-11-20, [platforms.md](platforms.md); since 2026-11-20 a macOS `.dmg` and a Windows `.zip` and installer are built and smoke-tested in CI, building and simulating a library project; real boards and releases left) | Spike first: build lcgamboa's QEMU library and `esp32bench-qemu-host` on macOS and Windows CI runners and run the hooks tests, then decide whether the setup step builds the simulator there or the app ships it prebuilt from CI. Then macOS, then Windows: device discovery without udev (IOKit; SetupAPI or polling), named pipes or loopback where Linux uses Unix sockets (Windows), the `esp32bench://` handler, installers (`.dmg`, `.msi` or similar), and CI that builds and smoke-tests each. Signing and notarisation need paid accounts, the owner's decision; until then the packages are unsigned. | spike 1–2 days; macOS 2–3 weeks; Windows 4–6 weeks |

Phase 4 builds on lcgamboa's library hooks, which phase 0 proved for GPIO and I2C. The work is mostly the canvas and the part models:

- **Host process:** grow `esp32bench-qemu-host` from the phase 0 spike into the per-session bridge. It fixes the bridged pins at start-up (the fork reads the pin map once) and streams pin events to the core, which forwards them to the UI.
- **Part models run in the host:** the I2C callback must answer synchronously, so sensors and other bus devices live in the host process, loaded from the part model files. The host answers for a whole bus, so it models every device on it.
- **Limits to design around:** GPIO outputs only for GPIO 0–31; the S3 had no hooks until patch 0015 (2026-11-19); the SPI hook exists but is untested.

## Testing

- **Core:** unit tests plus QEMU boot tests in CI for each supported chip and each pinned IDF and Arduino core version.
- **Library:** the same CI job that feeds the site's "runs in simulator" badge doubles as a regression suite.
- **Hardware in the loop:** one physical board per chip family on a USB hub, running flash, monitor and reset tests before each release.
- **UI:** smoke tests on Plasma Wayland and X11.
- **Packages:** every release's packages are installed into a fresh container of each supported distribution release and checked: commands, the core over its socket, the UI started headless, the udev rule's group, and the app's own arduino-cli install (`packaging/ci/smoke.sh`).

## Also plan for

- **First-run setup:** toolchains are several gigabytes. A setup wizard detects what is installed, shows download sizes, and supports several IDF and Arduino core versions side by side.
- **Translations:** the site ships in five languages, and so does the app (2026-11-19): English, Spanish, German, French and Brazilian Portuguese, in `ui/translations/` (Qt Linguist files compiled into the binary, loaded for the desktop's language). The core's own messages (errors, install hints, part names) are still English.
- **Signal trace view:** done 2026-11-19: a Trace dock draws the bridged pins in simulated time (QEMU's virtual clock), and the core exports the trace as VCD for PulseView (`sim.traceSave`, `sim run --trace`).
- **Part model format:** decided: a JSON description plus a Rhai script per model ([parts.md](parts.md)).
- **Project format:** a plain, git-friendly on-disk layout for local projects, compatible with the site's manifest.
- **Repo layout:** a separate repo from the site, with the shared schema published as its own package.

## Debian, Ubuntu and Fedora notes

- **arduino-cli:** no current package (Debian has it only in testing; Fedora not at all), so the app installs the pinned official release (1.5.1, the version Arch shipped when the library sweep ran) into `$XDG_DATA_HOME/esp32bench/arduino-cli/`, after checking its SHA-256. It needs no root.
- **Serial ports** belong to `dialout`, not `uucp`; the packages' udev rule uses it, and hints say so.
- **Qt:** Ubuntu 24.04 has Qt 6.4.2, so the UI builds against 6.4 or later. Qt's X11 platform plugin needs `libxcb-cursor0` (`xcb-util-cursor` on Fedora), which the distributions' Qt packages do not pull in, and Wayland desktops other than Plasma need `qt6-wayland` (`qt6-qtwayland`) to run natively; the packages depend on the first and recommend the second.
- **Rust:** the packages are built with rustup's stable toolchain; the distributions' own (Debian 13: 1.85) are older than the workspace needs.
- **Simulator build tools** have the usual names (`ninja-build`, `libslirp-dev` or `libslirp-devel`, and on Debian `python3-venv`, which QEMU's configure needs); the packages recommend them, and the app's setup step lists what is missing as an `apt` or `dnf` command.

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
