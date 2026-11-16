# esp32-desktop

A native Linux desktop workbench and simulator for ESP32 boards, built for KDE Plasma on Arch Linux. It is the desktop companion to [esp32bench.dev](https://esp32bench.dev/).

**Status:** planning. No code yet. Phase 0 (the QEMU spike) is next.

## What it will do

- Browse and build the esp32bench.dev project library with `arduino-cli` or `idf.py`.
- Flash, monitor and debug real boards.
- Run ESP32, ESP32-S3 and ESP32-C3 firmware in QEMU, with a console and GDB.

## How it is built

- **UI:** Qt 6 Widgets in C++.
- **Core:** a Rust daemon and CLI. The UI talks to it with JSON-RPC over a Unix socket.
- **Simulation:** Espressif's QEMU fork or lcgamboa's fork of it, to be chosen in phase 0.
- **Packaging:** a PKGBUILD for the AUR.

## Documents

- [docs/PLAN.md](docs/PLAN.md): the full plan, decisions, phases and risks.
- [CLAUDE.md](CLAUDE.md): working rules for Claude Code in this repo.

## Licence

GPL. The exact version (GPLv2-or-later or GPLv3) is chosen at the end of phase 0, because it depends on which QEMU fork the project links. A `LICENSE` file is added then.
