# esp32-desktop

Native Qt 6 workbench and simulator for ESP32 boards, targeting KDE Plasma on Arch Linux. Desktop companion to esp32bench.dev.

The plan is in `docs/PLAN.md`. Read it before any design work. It is the source of truth for scope, decisions and phase order.

## Current state

Phase 0 in progress; its checklist and exit criteria are in `docs/PHASE0.md`. `core/` has two crates: `esp32d-sim` (QEMU session supervisor, QMP, UART, GDB probe, image checks, pinned QEMU install) and the `esp32d` CLI. Espressif's QEMU fork boots ESP-IDF v6.0.2 and esp32bench.dev firmware on ESP32, S3 and C3, with results in `docs/qemu-forks.md`. The boot tests in `core/crates/sim/tests/boot.rs` boot `tests/firmware/out/hello-<chip>.bin` (built by `tests/firmware/build.sh` with `IDF_PATH` at the pinned IDF) and skip if QEMU or the images are missing. Still to do in phase 0: the lcgamboa fork comparison and the rest of the `docs/PHASE0.md` checklist.

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
| Configure UI | `cmake -S ui -B build/ui -G Ninja` |
| Build UI | `cmake --build build/ui` |
| Test UI | `ctest --test-dir build/ui --output-on-failure` |

## Decisions that are settled

Do not reopen these without the owner asking. Each has its reasoning in `docs/PLAN.md`.

- Qt 6 Widgets in C++ for the UI. Not QML, not GTK, not a webview.
- Rust core in a separate process. The UI never links Rust code.
- JSON-RPC over a Unix socket between UI and core.
- `arduino-cli` and `idf.py` are both first-class build systems.
- GPL licence. Version is picked at the end of phase 0.
- Arch Linux and Plasma are the only supported platform for v1.

## Decisions that are open

- Which QEMU fork: Espressif's or lcgamboa's. Phase 0 decides.
- GPLv2-or-later or GPLv3. Follows from the fork choice.
- QTermWidget or a plain text view for the serial monitor.

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
