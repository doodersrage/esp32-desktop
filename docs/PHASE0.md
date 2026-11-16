# Phase 0: QEMU spike

Phase 0 ends with one decision: which QEMU fork the project builds on. Everything here serves that decision.

- **Timebox:** 3 weeks at about 14 hours a week (about 42 hours). If the checklist is not done by then, decide with what is known and record the gaps.
- **Output:** a Rust CLI that boots firmware in QEMU, a results table for both forks, and the fork and licence decisions written into `docs/PLAN.md`.
- **Not in phase 0:** the daemon, JSON-RPC, any UI, flashing real boards.

## Before writing code

- [x] Install ESP-IDF and run `idf.py qemu monitor` on the stock `hello_world` example by hand. Confirms the toolchain and QEMU binaries work on this machine. *Done with `esp32bench` and `tests/firmware/hello` rather than `idf.py qemu`; IDF v6.0.2 is at `~/esp/esp-idf`.*
- [x] Install `arduino-cli` and the ESP32 Arduino core. Compile the Blink example for an ESP32 dev board. *arduino-cli 1.5.1 from Arch `extra`, core 3.3.12; compiled `tests/firmware/hello-arduino` instead of Blink.*
- [x] Record the exact versions used in the table below. These become the first pinned versions.
- [x] Decide the app name. Binary name, package name, socket path, config directory and URL scheme all derive from it. *`esp32bench`, matching esp32bench.dev and the `esp32bench://` scheme: CLI `esp32bench`, crates `esp32bench-*`, data in `~/.local/share/esp32bench`, runtime sockets in `$XDG_RUNTIME_DIR/esp32bench`, environment variables `ESP32BENCH_*`. The repo stays `esp32-desktop`.*

| Component | Version pinned |
| --- | --- |
| ESP-IDF | v6.0.2 (`tests/firmware/IDF_VERSION`) |
| ESP32 Arduino core | 3.3.12 (`tests/firmware/ARDUINO_ESP32_VERSION`) |
| Espressif QEMU (xtensa) | `esp-develop-9.2.2-20261109` (`core/crates/sim/src/pin.rs`) |
| Espressif QEMU (riscv32) | `esp-develop-9.2.2-20261109` (`core/crates/sim/src/pin.rs`) |
| lcgamboa QEMU (commit) | `885a6e7061fb70fac78a074a719423810bc19a17` (2026-11-14), built from source |
| Rust toolchain | 1.98.1 stable, edition 2024 |

## Test firmware

Keep these small and in `tests/firmware/`. Each prints a known marker string on UART0 so a boot test can match it.

- [x] `hello-idf`: ESP-IDF hello-world. *As `tests/firmware/hello`; also echoes UART0 input.*
- [x] `hello-arduino`: Arduino sketch that prints over `Serial`. *Also echoes input.*
- [x] `gpio-blink`: toggles one GPIO and prints each state change. *Combined with `gpio-input` as `tests/firmware/gpio`.*
- [x] `gpio-input`: prints when an input pin changes.
- [x] `i2c-scan`: scans the I2C bus and prints what it finds. *Then reads two registers from 0x3C.*
- [x] `wifi-sta` (added): joins an open emulated network and prints its IP.

## The CLI

One binary in `core/` that takes a chip and a flash image, and can:

- [x] Launch QEMU as a child process and kill it reliably on exit, including on Ctrl-C and on panic.
- [x] Stream UART0 from a Unix socket to stdout, as bytes.
- [x] Reset the chip over QMP.
- [x] Start QEMU waiting for GDB, and attach the toolchain GDB with the project's ELF.
- [x] Run as a boot test: exit 0 when a marker string appears within a timeout, non-zero on timeout or panic banner.

## Fork checklist

Run every row against both forks and fill in the result. Use "yes", "no" or "partial" with a short note.

| Check | Espressif fork | lcgamboa fork |
| --- | --- | --- |
| Boots `hello-idf` on ESP32 with the pinned IDF | yes | yes, on `esp32-picsimlab`; plain `esp32` aborts at start-up |
| Boots `hello-idf` on ESP32-S3 | yes | yes |
| Boots `hello-idf` on ESP32-C3 | yes | yes, on `esp32c3` and `esp32c3-picsimlab` |
| Boots `hello-arduino` on ESP32 with the pinned Arduino core | yes, all three chips, core 3.3.12 | yes, all three chips, core 3.3.12. Older core 2.x (IDF 4.4.7) builds fail flash init on S3 and C3 |
| UART0 output readable over a socket | yes | yes |
| UART0 input works (type into the firmware) | yes | yes |
| Reset from the host works | yes, QMP `system_reset` | yes, QMP `system_reset` |
| GDB attach, breakpoint and step work | yes, all three chips | yes, all three chips |
| Host can read a GPIO output (`gpio-blink`) | no hook | yes on ESP32 and C3, through `esp32bench-qemu-host` and the library build; GPIO 0–31 only. No S3 |
| Host can drive a GPIO input (`gpio-input`) | no hook | yes on ESP32 and C3, through `esp32bench-qemu-host`. No S3 |
| Host can answer as an I2C device (`i2c-scan`) | no hook (ESP32 has a fixed built-in TMP105 at 0x48) | yes on ESP32 and C3, through `esp32bench-qemu-host`: the scan finds only the modelled device and register reads return its values. Callback is synchronous, and the host answers for the whole bus |
| Wi-Fi station connects to an emulated network | no: panics on ESP32, no IP on S3/C3 | yes on ESP32 and C3 (C3 needs an eFuse image); no on S3 |
| Runs at roughly real-time speed | yes: 1.00 wall s per guest s, idle firmware | yes: 1.00 wall s per guest s, idle firmware |
| Can be driven as a separate process (not only as a linked library) | yes | yes: executables for most rows; the library through `esp32bench-qemu-host`, one process per session |
| Builds from source on Arch without patches | not tried (prebuilt binaries used) | yes, both as executables and with `build_libqemu-esp32.sh` |
| Upstream had commits in the last 6 months | yes, 2026-11-06 | yes, 2026-11-14 |

## Decision rule

Choose the fork that passes more of the first eight rows (boot, UART, reset, GDB) on the pinned versions. Those rows are phases 1 to 3 and they are required.

If both pass those, prefer the fork with working GPIO and I2C hooks, because that decides whether phase 4 is mostly UI work or mostly QEMU work.

If neither fork boots current IDF and Arduino builds on all three chips, stop and raise it. That changes the plan's scope.

## When phase 0 is done

Follow the `update-plan` skill. In short:

- [x] Fill in both tables above and commit this file.
- [x] Record the fork choice in "Key decisions" in `docs/PLAN.md` and move it to "settled" in `CLAUDE.md`. *lcgamboa's fork.*
- [x] Set the GPL version, add the `LICENSE` file, and update the README. *GPLv2-or-later.*
- [x] Rewrite phase 4 in the plan to match the chosen fork.
- [x] Correct anything in `.claude/skills/qemu-esp32/SKILL.md` that turned out wrong, and remove its "verify" notes.
- [x] Note actual hours spent next to the phase 0 estimate. *About 4 hours, 2026-11-15.*
