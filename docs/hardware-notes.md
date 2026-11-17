# Notes from real boards

What real hardware does that tests on QEMU and pseudo-terminals cannot show. Add to this file whenever a board surprises us.

## ESP32-C3 with built-in USB (303a:1001)

Seen 2026-11-16 on an ESP32-C3 running ProbeHarbor firmware 1.6.1, on `/dev/ttyACM0`.

- **Opening the port restarts the chip.** Every open shows a fresh boot log. Linux raises DTR and RTS while opening a tty, before any program can change them, and the USB-Serial-JTAG peripheral resets the chip on that line state. The core switches both lines off right after opening, so the chip boots its app rather than the ROM downloader, but the reset itself cannot be avoided from user space. The Arduino IDE behaves the same. The serial monitor says so when it opens such a port.
- **device.list cannot tell S3, C3 and C6 apart** from the USB IDs alone (all are 303a:1001). `board.inspect` reads the chip from the ROM.

- **Closing a port can block for 30 seconds.** If bytes were written that the firmware never reads (the hello firmware reads UART0, not the USB port), Linux waits for them to drain when the port is closed (`closing_wait`). The serial monitor hung on exit until the core started discarding unsent output before closing. It also no longer drains (`tcdrain`) after each write, for the same reason.

### Board operations on the C3 (2026-11-16)

All through `esp32bench board ...`, with the USB-Serial-JTAG reset sequence and espflash's stub:

| Operation | Result |
| --- | --- |
| inspect | ESP32-C3 v0.4, 40 MHz, 4 MiB, MAC from eFuse, secure boot / flash encryption / secure download all off, six partitions read. The board went back to its firmware afterwards. |
| read (backup) | 4 MiB in 37 s. Two reads differed only in 28 bytes of the `nvs` partition (0xC000), which the firmware rewrites as it runs. |
| flash (hello firmware) | 229 KiB written and MD5-checked in 2.3 s; the board rebooted into it (`esp32bench-hello ready target=esp32c3`). |
| flash --whole (restore) | 4 MiB in 18 s; read back, it matched the backup apart from the same `nvs` bytes, and ProbeHarbor 1.6.1 booted again. |

The backup is kept at `~/esp/backups/c3-probeharbor-1.6.1-full.bin` on the owner's machine (sha256 `c01c1915…`).

## Plasma check (2026-11-16)

The owner ran the app on KDE Plasma (Wayland, Breeze dark), with the core from a dev build and the project library from a local site build (`ESP32BENCH_MANIFEST`, since the site's `desktop-manifest` branch is not deployed yet):

- The window follows the dark theme; the docks, menus and status bar lay out as intended.
- Start core connected; the Devices dock listed the C3 on `/dev/ttyACM0`.
- Inspect showed the C3's chip, MAC, security state and partition table, matching the CLI.
- The library listed all 54 projects, and Blink built for ESP32 with the real `arduino-cli` and core 3.3.12.
- That build, debugged on lcgamboa's fork from the CLI with the Arduino core's GDB: a hardware breakpoint in `setup()` stopped at `blink.ino` line 4, and stepping and the call stack (`loopTask` in the core's `main.cpp`) worked.

- In the app, Debug in simulator on that build stopped at breakpoint 1 in `setup` (`blink.ino` line 4), with the call stack and registers.
- Flash to board with the C3 selected refused the ESP32 build: "the board is an esp32c3, but this build is for esp32". Nothing was written.

Seen while checking: there is no app icon yet (packaging). Fixed since: the core had to be started with the Start core button each time (the app now starts it once on launch if none is running); the Debugger opened as a tab over the Simulator console (it now sits beside it, and a saved layout with the tab is corrected when debugging starts), and the Devices table showed a horizontal scrollbar (the USB device column now stretches and elides).

## Phase 4 check on Plasma (2026-11-17)

The owner placed every built-in part from the Circuit dock without an error. rtc-clock with a DS3231 ran in the simulator, and its `set` command changed the clock. bme280-web cannot be checked through: it finds the BME280, then waits forever for Wi-Fi, and simulations have no network (rule 4). A serial-only project, BME280 weather readings (`bme280-serial`, on the site's `desktop-manifest` branch), was added for this, and the owner checked it: the readings followed the Circuit dock's controls, and a second run started where they were left. Seen while checking: after Stop, starting the same build again meant going back to the Build dock (it is tabbed with the Simulator); the Simulator dock now has Run again.

## Erase and restore on the ESP32-C3 (2026-11-17)

With the owner's approval, on the C3 at `/dev/ttyACM0` (ESP32-C3 v0.4, 4 MiB), from the CLI:

1. `board read` twice: the whole flash both times (37 s each), identical except 241 bytes of NVS, which the firmware rewrites when each read resets it. The board had been reflashed elsewhere since the 2026-11-16 backup: ProbeHarbor 1.5.2 in app0 and app1 blank, where the backup has 1.6.1 and a 1.4.1-test build. Both copies are kept in `~/esp/backups/`.
2. `board erase --yes`: 2 s with the stub. Read back, all 4,194,304 bytes were 0xFF.
3. `board flash --whole` with the second read: 14 s. Read back, identical to it byte for byte, NVS included.
4. On reset it booted ProbeHarbor 1.5.2 as before.

So erase, whole-image restore and read all work on real hardware through espflash's stub. The daemon's `board.erase` uses the same library call but has not been run against a real board.

## The C3 through a PL2303 USB-UART adapter (2026-11-17)

A Prolific PL2303 cable (`067b:2303`) wired to the C3's UART0: 5V, GND, adapter TX to GPIO20, RX to GPIO21. Such cables have no DTR or RTS, so nothing can reset the board or select download mode: the owner held BOOT and tapped RESET by hand. (A first attempt with the cable's GND on 3V3 received nothing.)

- The ROM's banner arrived over the adapter: `boot:0x7 (DOWNLOAD(USB/UART0/1))`, `waiting for download`.
- `board inspect --no-reset` read the same chip, MAC and partition table as over USB, in 1.2 s.
- `board read --no-reset` read all 4 MiB in 6 min 19 s at 115200 baud. Every region nothing had written to (bootloader, partition table, app0, spiffs) matched the copy restored that morning byte for byte; app1 had meanwhile received ProbeHarbor 1.6.1 from the firmware itself, with otadata switched to it.
- The core did not know the PL2303: `device list` called it unknown, and the udev rule did not cover it. Both fixed; the RPC protocol is 0.18.0 for the new `pl2303` bridge value.

Still untried: the automatic DTR/RTS reset of boards with an auto-reset circuit (CP210x or CH340 dev boards), and writing over a UART bridge.

## Not yet tried on real hardware

- The DTR/RTS reset sequence used by UART-bridge boards (CP210x, CH340); only the USB-Serial-JTAG sequence has been exercised.
- The serial monitor's Reset board button, and the daemon and UI paths for board jobs (the CLI uses the same library calls).
