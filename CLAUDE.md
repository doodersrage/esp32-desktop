# esp32-desktop

Native Qt 6 workbench and simulator for ESP32 boards, targeting KDE Plasma on Arch Linux. Desktop companion to esp32bench.dev.

The plan is in `docs/PLAN.md`. Read it before any design work. It is the source of truth for scope, decisions and phase order.

## Current state

Phases 1 and 2 are feature complete, awaiting a check on real hardware and Plasma; phase 3 (simulator) is in progress: builds run in QEMU from the app, with console and reset, and can be debugged there with GDB (breakpoints, stepping, stack, registers); lcgamboa's fork is built from source at the pinned commit by the setup step (`toolchain install qemu-lcgamboa`), and simulations run on it once it is built (ESP32 and C3 through `esp32bench-qemu-host` and the fork's library, the S3 on its executable; `esp32bench-sim::engine`), else on Espressif's prebuilt QEMU; `sim.start` reports which. Sessions never get a network (`-nic none`). The framebuffer is still to do. Phase 0 is done (2026-11-15); its results are in `docs/PHASE0.md` and `docs/qemu-forks.md`. The RPC protocol is in `docs/rpc.md` and the method definitions in `schema/rpc/`. `core/` crates: `esp32bench-rpc` (JSON-RPC over a Unix socket: router, server, client), `esp32bench-daemon` (`esp32benchd`, serving `core.hello`, `core.unsubscribe`, `job.cancel`, `device.list`, `device.watch` (events: `device.changed`), `library.list`, `library.get`, `library.refresh` (job, event: `library.refreshed`) `build.start` (consent token, job, events: `build.log`, `build.finished`), `serial.open`/`serial.write`/`serial.reset` (events: `serial.data`, `serial.closed`), `toolchain.status`/`toolchain.install` (job, events: `toolchain.progress`, `toolchain.finished`), `board.inspect`/`board.flash`/`board.erase` (jobs, events: `board.progress`, `board.finished`; flashing needs consent), `sim.start`/`sim.write`/`sim.reset` (events: `sim.output`, `sim.exited`), and with `sim.start`'s `debug: true`, `debug.break`/`debug.delete`/`debug.control`/`debug.inspect` (events: `debug.stopped`, `debug.running`, `debug.exited`)), `esp32bench-device` (serial port discovery with chip hints, and a udev hotplug monitor), `esp32bench-library` (fetches, checks and caches the esp32bench.dev project manifest; format in `schema/project-manifest/`), `esp32bench-build` (build plans, consent tokens and `arduino-cli` runs in a private sketchbook), `esp32bench-serial` (open ports, raw bytes, esptool-style reset), `esp32bench-toolchain` (detects arduino-cli, the pinned ESP32 core, QEMU, the lcgamboa fork build and board access, download sizes, installs what needs no root), `esp32bench-flash` (espflash as a library: chip info, eFuse security, partition table, write, erase; tested against QEMU in download mode), `esp32bench-sim` (QEMU sessions), `esp32bench-debug` (GDB/MI client; uses the GDB the pinned Arduino core installs), `esp32bench` (the CLI) and `esp32bench-qemu-host` (lcgamboa's QEMU library with GPIO and I2C hooks). Simulator methods are not on the RPC API yet. `ui/` has the first Qt shell: `RpcClient` (async JSON-RPC over `QLocalSocket`), `CoreConnection` (handshake, reconnect, starting `esp32benchd`, typed requests with result signals; widgets use this, never raw RPC), `MainWindow`, a Devices dock that updates live through `device.watch`, in the centre a Library panel beside a ProjectView (parts, wiring, code and expected output per board, and a Build button), a Build dock with the log, a Serial monitor dock (double-click a port in Devices to open it), a consent dialog before any build, a Toolchain setup dialog (Tools menu; opens by itself once if build tools are missing), Flash to board and Run in simulator in the Build dock, a Simulator dock (console, input, reset, stop; shares ConsoleView with the serial monitor), a Debugger dock beside it (Debug in simulator in the Build dock: run controls, breakpoints, call stack, registers), and Inspect and Erase in the Devices panel's context menu. The udev rule is `packaging/udev/69-esp32bench.rules`. The site side of the project library lives on the unmerged `desktop-manifest` branch of the esp32-bench repo; until it is deployed, point the core at a local site build with `ESP32BENCH_MANIFEST=<esp32-bench>/dist/desktop-manifest.json`. To run it against a dev build: `ESP32BENCH_DAEMON=core/target/debug/esp32benchd build/ui/esp32bench-gui`. `ui/tests/tst_integration` runs against the real daemon when `ESP32BENCH_DAEMON` is set and skips otherwise. Boot tests in `core/crates/sim/tests/boot.rs` and GDB tests in `core/crates/debug/tests/qemu.rs` use images (and ELFs in `tests/firmware/build/`) from `tests/firmware/out/` (built by `tests/firmware/build.sh`) and skip without QEMU; `core/crates/qemu-host/tests/hooks.rs` uses `ESP32BENCH_TEST_QEMU_LIB_{XTENSA,RISCV32}` if set, else the installed fork build. `esp32bench qemu install` still fetches Espressif's prebuilt QEMU, the fallback until the fork is built and what the flash tests' download mode uses; `esp32bench sim run --engine lcgamboa|espressif` picks one. The daemon finds `esp32bench-qemu-host` next to its own executable (or `ESP32BENCH_QEMU_HOST`), so packaging must install it beside `esp32benchd`. The hooks tests and the `lcgamboa_*` boot tests use the installed fork build when no variable is set.

CI is in `.github/workflows/ci.yml`. It runs fmt, clippy and tests for the core; the UI job skips until `ui/CMakeLists.txt` exists. CI has no QEMU, so the boot tests skip there.

The commands and paths below are the intended layout. If one does not exist yet, create it as described here; do not invent a different structure.

## Repo layout

| Path | Contents |
| --- | --- |
| `core/` | Rust workspace: daemon, CLI and library crates |
| `ui/` | Qt 6 Widgets app, C++ and CMake |
| `schema/` | JSON schemas shared with the esp32bench.dev site, and the JSON-RPC method definitions |
| `packaging/` | PKGBUILDs, `.desktop` file, udev rule |
| `docs/` | Plan and design notes |
| `tests/firmware/` | Small firmware projects used by QEMU boot tests |

## Commands

| Task | Command |
| --- | --- |
| Build core | `cargo build --manifest-path core/Cargo.toml` |
| Test core | `cargo test --manifest-path core/Cargo.toml` |
| Lint core | `cargo clippy --manifest-path core/Cargo.toml --all-targets -- -D warnings` |
| Format core | `cargo fmt --manifest-path core/Cargo.toml --all` |
| Run the daemon | `cargo run --manifest-path core/Cargo.toml --bin esp32benchd` |
| Call it | `cargo run --manifest-path core/Cargo.toml --bin esp32bench -- rpc core.hello` |
| Configure UI | `cmake -S ui -B build/ui -G Ninja` |
| Build UI | `cmake --build build/ui` |
| Test UI | `ctest --test-dir build/ui --output-on-failure` |

## Decisions that are settled

Do not reopen these without the owner asking. Each has its reasoning in `docs/PLAN.md`.

- Qt 6 Widgets in C++ for the UI. Not QML, not GTK, not a webview.
- Rust core in a separate process. The UI never links Rust code.
- JSON-RPC over a Unix socket between UI and core.
- `arduino-cli` and `idf.py` are both first-class build systems.
- GPLv2-or-later. QEMU is GPLv2, so GPLv3 is ruled out.
- lcgamboa's QEMU fork for the simulator, pinned to a commit and built from source. GPIO and I2C go through its library build in a per-session host process.
- The app name is `esp32bench` (CLI, crates, data and runtime directories, `ESP32BENCH_*` variables). The repo stays `esp32-desktop`.
- Arch Linux and Plasma are the only supported platform for v1.
- The serial monitor is a plain text view (`QPlainTextEdit`), not QTermWidget.

## Decisions that are open

None at the moment.

When work resolves one of these, update `docs/PLAN.md` in the same change.

## Working rules

1. **Core first.** Every feature lands in the core with a JSON-RPC method and a test before any UI is written for it. The CLI must be able to do everything the UI can.
2. **No logic in the UI.** The UI renders state and sends requests. Serial parsing, build orchestration, QEMU control and file formats belong in the core.
3. **Schema changes are shared changes.** Files in `schema/` are consumed by the esp32bench.dev site. Never make a breaking change without bumping the schema version and noting it in the commit message.
4. **Nothing runs without consent.** Anything opened from an `esp32bench://` link or a downloaded project is untrusted. The app must ask before building or flashing it. Do not add a path that skips the prompt.
5. **No root.** Device access comes from the udev rule and the `uucp` group. Never add code that calls `sudo` or `pkexec`.
6. **Simulator claims stay honest.** The simulator is firmware-level. Do not describe an unsupported peripheral or board as working. Boards outside ESP32, S3 and C3 are workbench-only.
7. **Translatable strings.** Every user-visible string in the UI goes through `tr()`.
8. **Pin versions.** QEMU builds, IDF versions and Arduino core versions used in tests are pinned in the repo, never "latest".
9. **Stay in v1 scope.** No built-in code editor, analog simulation, Windows or macOS support, or Flatpak. Raise it with the owner instead of building it.

## Licensing

The project is GPL. Code copied or adapted from PICSimLab, QEMU or other GPL projects must keep its copyright notices and be noted in the commit message. Do not bring in code under AGPL (Velxio) or any proprietary licence (Wokwi).

## More detailed rules

Path-specific rules are in `.claude/rules/`. Task guides are in `.claude/skills/`.
