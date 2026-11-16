# esp32-desktop

A native Linux desktop workbench and simulator for ESP32 boards, built for KDE Plasma on Arch Linux. It is the desktop companion to [esp32bench.dev](https://esp32bench.dev/).

**Status:** phase 0 (the QEMU spike) is done: a Rust CLI boots ESP-IDF and Arduino firmware in QEMU on ESP32, S3 and C3, and lcgamboa's QEMU fork was chosen. Phase 1 (the core daemon and the first Qt shell) is next.

## What it will do

- Browse and build the esp32bench.dev project library with `arduino-cli` or `idf.py`.
- Flash, monitor and debug real boards.
- Run ESP32, ESP32-S3 and ESP32-C3 firmware in QEMU, with a console and GDB.

## How it is built

- **UI:** Qt 6 Widgets in C++.
- **Core:** a Rust daemon and CLI. The UI talks to it with JSON-RPC over a Unix socket.
- **Simulation:** [lcgamboa's QEMU fork](https://github.com/lcgamboa/qemu), which adds Wi-Fi and GPIO and I2C hooks to Espressif's QEMU.
- **Packaging:** a PKGBUILD for the AUR.

## Documents

- [docs/PLAN.md](docs/PLAN.md): the full plan, decisions, phases and risks.
- [docs/PHASE0.md](docs/PHASE0.md): checklist and exit criteria for the QEMU spike.
- [CLAUDE.md](CLAUDE.md): working rules for Claude Code in this repo.

## Licence

GPLv2-or-later. See [LICENSE](LICENSE) for the GPLv2 text. The simulator loads lcgamboa's QEMU library, and QEMU is GPLv2, so the project cannot move to GPLv3-only.
