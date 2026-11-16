# Phase 0: QEMU spike

Phase 0 ends with one decision: which QEMU fork the project builds on. Everything here serves that decision.

- **Timebox:** 3 weeks at about 14 hours a week (about 42 hours). If the checklist is not done by then, decide with what is known and record the gaps.
- **Output:** a Rust CLI that boots firmware in QEMU, a results table for both forks, and the fork and licence decisions written into `docs/PLAN.md`.
- **Not in phase 0:** the daemon, JSON-RPC, any UI, flashing real boards.

## Before writing code

- [ ] Install ESP-IDF and run `idf.py qemu monitor` on the stock `hello_world` example by hand. Confirms the toolchain and QEMU binaries work on this machine.
- [ ] Install `arduino-cli` and the ESP32 Arduino core. Compile the Blink example for an ESP32 dev board.
- [ ] Record the exact versions used in the table below. These become the first pinned versions.
- [ ] Decide the app name. Binary name, package name, socket path, config directory and URL scheme all derive from it.

| Component | Version pinned |
| --- | --- |
| ESP-IDF | |
| ESP32 Arduino core | |
| Espressif QEMU (xtensa) | |
| Espressif QEMU (riscv32) | |
| lcgamboa QEMU (commit) | |
| Rust toolchain | |

## Test firmware

Keep these small and in `tests/firmware/`. Each prints a known marker string on UART0 so a boot test can match it.

- [ ] `hello-idf`: ESP-IDF hello-world.
- [ ] `hello-arduino`: Arduino sketch that prints over `Serial`.
- [ ] `gpio-blink`: toggles one GPIO and prints each state change.
- [ ] `gpio-input`: prints when an input pin changes.
- [ ] `i2c-scan`: scans the I2C bus and prints what it finds.

## The CLI

One binary in `core/` that takes a chip and a flash image, and can:

- [ ] Launch QEMU as a child process and kill it reliably on exit, including on Ctrl-C and on panic.
- [ ] Stream UART0 from a Unix socket to stdout, as bytes.
- [ ] Reset the chip over QMP.
- [ ] Start QEMU waiting for GDB, and attach the toolchain GDB with the project's ELF.
- [ ] Run as a boot test: exit 0 when a marker string appears within a timeout, non-zero on timeout or panic banner.

## Fork checklist

Run every row against both forks and fill in the result. Use "yes", "no" or "partial" with a short note.

| Check | Espressif fork | lcgamboa fork |
| --- | --- | --- |
| Boots `hello-idf` on ESP32 with the pinned IDF | | |
| Boots `hello-idf` on ESP32-S3 | | |
| Boots `hello-idf` on ESP32-C3 | | |
| Boots `hello-arduino` on ESP32 with the pinned Arduino core | | |
| UART0 output readable over a socket | | |
| UART0 input works (type into the firmware) | | |
| Reset from the host works | | |
| GDB attach, breakpoint and step work | | |
| Host can read a GPIO output (`gpio-blink`) | | |
| Host can drive a GPIO input (`gpio-input`) | | |
| Host can answer as an I2C device (`i2c-scan`) | | |
| Wi-Fi station connects to an emulated network | | |
| Runs at roughly real-time speed | | |
| Can be driven as a separate process (not only as a linked library) | | |
| Builds from source on Arch without patches | | |
| Upstream had commits in the last 6 months | | |

## Decision rule

Choose the fork that passes more of the first eight rows (boot, UART, reset, GDB) on the pinned versions. Those rows are phases 1 to 3 and they are required.

If both pass those, prefer the fork with working GPIO and I2C hooks, because that decides whether phase 4 is mostly UI work or mostly QEMU work.

If neither fork boots current IDF and Arduino builds on all three chips, stop and raise it. That changes the plan's scope.

## When phase 0 is done

Follow the `update-plan` skill. In short:

- [ ] Fill in both tables above and commit this file.
- [ ] Record the fork choice in "Key decisions" in `docs/PLAN.md` and move it to "settled" in `CLAUDE.md`.
- [ ] Set the GPL version, add the `LICENSE` file, and update the README.
- [ ] Rewrite phase 4 in the plan to match the chosen fork.
- [ ] Correct anything in `.claude/skills/qemu-esp32/SKILL.md` that turned out wrong, and remove its "verify" notes.
- [ ] Note actual hours spent next to the phase 0 estimate.
