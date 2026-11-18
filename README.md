# esp32-desktop

A native Linux desktop workbench and simulator for ESP32 boards, built for KDE Plasma on Arch Linux. It is the desktop companion to [esp32bench.dev](https://esp32bench.dev/).

**Status:** version 0.1.2 is out, for Arch Linux with KDE Plasma only. It browses and builds the esp32bench.dev library, flashes, inspects and monitors real boards, and runs builds in a simulator. The automatic reset of boards with a USB-UART bridge (CP210x, CH340) is still untested on hardware.

## What it does

- Browse the esp32bench.dev project library and build projects with `arduino-cli`, asking before anything downloaded is built or flashed. Links on esp32bench.dev open a project in the app.
- Flash, inspect, erase and monitor real boards over USB, without root.
- Run ESP32, ESP32-S3 and ESP32-C3 builds in a simulator (lcgamboa's QEMU fork, built on your computer by the setup step), with a console, reset, GDB debugging and the virtual display. Of the library's 227 simulated builds, 222 print what they print on a real board; the rest use Bluetooth.
- Place virtual parts on the ESP32 and C3: LEDs, buttons, switches, an RGB LED, a NeoPixel strip, an SSD1306 OLED, and BME280, MPU-6050 and DS3231 modules on I2C.
- Give simulated sketches (ESP32, S3 and C3) a Wi-Fi network that reaches nothing outside: they get an address, and their web pages open on your computer. Library sketches are joined to it for you.

The simulator is firmware-level: no Bluetooth, no analog inputs (they read 0), and no encrypted Wi-Fi networks. Light and deep sleep wake on a timer only.

Report problems at [github.com/doodersrage/esp32-desktop/issues](https://github.com/doodersrage/esp32-desktop/issues).

## How it is built

- **UI:** Qt 6 Widgets in C++.
- **Core:** a Rust daemon and CLI. The UI talks to it with JSON-RPC over a Unix socket.
- **Simulation:** [lcgamboa's QEMU fork](https://github.com/lcgamboa/qemu), which adds Wi-Fi and GPIO and I2C hooks to Espressif's QEMU.
- **Packaging:** PKGBUILDs (`packaging/`), and a pacman package with each release.

## Documents

- [docs/PLAN.md](docs/PLAN.md): the full plan, decisions, phases and risks.
- [docs/PHASE0.md](docs/PHASE0.md): checklist and exit criteria for the QEMU spike.
- [CLAUDE.md](CLAUDE.md): working rules for Claude Code in this repo.

## Installing

On Arch Linux, install the package from the [latest release](https://github.com/doodersrage/esp32-desktop/releases/latest):

```sh
curl -LO https://github.com/doodersrage/esp32-desktop/releases/download/v0.1.2/esp32bench-bin-0.1.2-1-x86_64.pkg.tar.zst
sudo pacman -U esp32bench-bin-0.1.2-1-x86_64.pkg.tar.zst
```

pacman installs the dependencies from the official repos. The package is not signed, so it is installed from the downloaded file. To build it yourself, run `makepkg -si` in `packaging/esp32bench-bin/` (the release's files) or `packaging/esp32bench-git/` (the latest source). The packages are not on the AUR: its registration is closed for now.

It installs `esp32bench-gui` (the app, also in the Plasma launcher as ESP32 Bench), `esp32benchd` (the core it starts), the `esp32bench` CLI, the udev rule for USB serial boards, and the handler for `esp32bench://` links from esp32bench.dev. To reach boards without root, be in the `uucp` group or logged in at the seat (the rule uses `uaccess`), then replug the board. Builds need `arduino-cli`; the simulator is built from source by the app's setup step (Tools menu), which lists any build tools it is missing.

## Licence

GPLv2-or-later. See [LICENSE](LICENSE) for the GPLv2 text. The simulator loads lcgamboa's QEMU library, and QEMU is GPLv2, so the project cannot move to GPLv3-only.
