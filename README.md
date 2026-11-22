# esp32-desktop

A native Linux desktop workbench and simulator for ESP32 boards, for Arch, Debian, Ubuntu and Fedora. It is the desktop companion to [esp32bench.dev](https://esp32bench.dev/).

**Status:** version 0.1.10 is out, with packages for Arch, Debian 13, Ubuntu 24.04 and 26.04, Fedora 44 and 45, macOS (Apple silicon) and Windows (x86-64), each built and tested on its system by CI; real boards have been checked on Arch only so far. It browses and builds the esp32bench.dev library and your own ESP-IDF projects, flashes, inspects and monitors real boards, and runs builds in a simulator. The automatic reset of boards with a USB-UART bridge (CP210x, CH340) is still untested on hardware.

## What it does

- Browse the esp32bench.dev project library and build projects with `arduino-cli`, asking before anything downloaded is built or flashed. Links on esp32bench.dev open a project in the app.
- Open your own ESP-IDF project folders and build them with your installed ESP-IDF (`idf.py`) for the ESP32, S3 or C3, then flash, monitor, simulate and debug them the same way.
- Flash, inspect, erase and monitor real boards over USB, without root.
- Run ESP32, ESP32-S3 and ESP32-C3 builds in a simulator (lcgamboa's QEMU fork, built on your computer by the setup step), with a console, reset, GDB debugging and the virtual display. Of the library's 227 simulated builds, 222 print what they print on a real board; the rest use Bluetooth.
- Place virtual parts on the ESP32, S3 and C3: LEDs, buttons, switches, an RGB LED, a NeoPixel strip, an SSD1306 OLED, and BME280, MPU-6050 and DS3231 modules on I2C.
- Trace the simulated pins like a logic analyzer, in simulated time, and save the trace as a VCD file for PulseView or GTKWave.
- Give simulated sketches (ESP32, S3 and C3) a Wi-Fi network that reaches nothing outside: they get an address, and their web pages open on your computer. Library sketches are joined to it for you.
- In English, Spanish, German, French and Brazilian Portuguese, following the desktop's language.

The simulator is firmware-level: no Bluetooth and no encrypted Wi-Fi networks. Analog inputs read the voltage of a potentiometer part placed on them. Light and deep sleep wake on a timer only.

Report problems at [github.com/doodersrage/esp32-desktop/issues](https://github.com/doodersrage/esp32-desktop/issues).

## How it is built

- **UI:** Qt 6 Widgets in C++.
- **Core:** a Rust daemon and CLI. The UI talks to it with JSON-RPC over a Unix socket.
- **Simulation:** [lcgamboa's QEMU fork](https://github.com/lcgamboa/qemu), which adds Wi-Fi and GPIO and I2C hooks to Espressif's QEMU.
- **Packaging:** PKGBUILDs, Debian and Fedora package scripts (`packaging/`); each release has a pacman package, `.deb`s and `.rpm`s, built and install-tested in CI.

## Documents

- [docs/PLAN.md](docs/PLAN.md): the full plan, decisions, phases and risks.
- [docs/PHASE0.md](docs/PHASE0.md): checklist and exit criteria for the QEMU spike.
- [CLAUDE.md](CLAUDE.md): working rules for Claude Code in this repo.

## Installing

Packages are attached to each [release](https://github.com/doodersrage/esp32-desktop/releases/latest). They are not signed, so they are installed from the downloaded file.

**Arch Linux:**

```sh
curl -LO https://github.com/doodersrage/esp32-desktop/releases/download/v0.1.10/esp32bench-bin-0.1.10-1-x86_64.pkg.tar.zst
sudo pacman -U esp32bench-bin-0.1.10-1-x86_64.pkg.tar.zst
```

To build it yourself, run `makepkg -si` in `packaging/esp32bench-bin/` (the release's files) or `packaging/esp32bench-git/` (the latest source). The packages are not on the AUR: its registration is closed for now.

**Debian 13, Ubuntu 24.04 and 26.04, Fedora 44 and 45:** download the file for your release, `esp32bench_0.1.10-1.debian13_amd64.deb`, `….ubuntu24.04_amd64.deb`, `….ubuntu26.04_amd64.deb`, or `esp32bench-0.1.10-1.fc44.x86_64.rpm` / `….fc45.x86_64.rpm`, and install it with your package manager, which fetches the dependencies. For example:

```sh
curl -LO https://github.com/doodersrage/esp32-desktop/releases/download/v0.1.10/esp32bench_0.1.10-1.ubuntu26.04_amd64.deb
sudo apt install ./esp32bench_0.1.10-1.ubuntu26.04_amd64.deb

curl -LO https://github.com/doodersrage/esp32-desktop/releases/download/v0.1.10/esp32bench-0.1.10-1.fc45.x86_64.rpm
sudo dnf install ./esp32bench-0.1.10-1.fc45.x86_64.rpm
```

Each package is built on its own release and installed and checked there by CI before it is attached. Real boards have been tested on Arch so far.

**macOS 13 or later, Apple silicon:** open `esp32bench-0.1.10-macos-arm64.dmg` and drag ESP32 Bench to Applications. The app is not notarised, so the first time macOS refuses to open it: open System Settings → Privacy & Security and choose Open Anyway. The simulator is included.

**Windows 10 and 11, x86-64:** run `esp32bench-0.1.10-windows-x86_64-setup.exe`, which installs for your user without administrator rights, or unpack `esp32bench-0.1.10-windows-x86_64.zip` anywhere. The files are not signed, so SmartScreen warns: choose More info → Run anyway. The simulator is included. To build your own ESP-IDF projects, install ESP-IDF with Espressif's installer; the app finds it.

On macOS and Windows, real boards (the serial monitor and flashing) are not yet tested.

Every Linux package installs `esp32bench-gui` (the app, in the desktop's launcher as ESP32 Bench), `esp32benchd` (the core it starts), the `esp32bench` CLI, the udev rule for USB serial boards, and the handler for `esp32bench://` links from esp32bench.dev. To reach boards without root, be logged in at the seat (the rule uses `uaccess`) or in the group that owns serial ports (`uucp` on Arch, `dialout` on Debian, Ubuntu and Fedora), then replug the board. Builds need `arduino-cli`: on Arch it is a package; elsewhere the app's Toolchain setup installs a pinned release into its own folder. The simulator is built from source by the same setup step (Tools menu), which lists any build tools it is missing as a command for your distribution.

## Licence

GPLv2-or-later. See [LICENSE](LICENSE) for the GPLv2 text. The simulator loads lcgamboa's QEMU library, and QEMU is GPLv2, so the project cannot move to GPLv3-only.
