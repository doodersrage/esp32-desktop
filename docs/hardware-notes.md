# Notes from real boards

What real hardware does that tests on QEMU and pseudo-terminals cannot show. Add to this file whenever a board surprises us.

## ESP32-C3 with built-in USB (303a:1001)

Seen 2026-11-16 on an ESP32-C3 running ProbeHarbor firmware 1.6.1, on `/dev/ttyACM0`.

- **Opening the port restarts the chip.** Every open shows a fresh boot log. Linux raises DTR and RTS while opening a tty, before any program can change them, and the USB-Serial-JTAG peripheral resets the chip on that line state. The core switches both lines off right after opening, so the chip boots its app rather than the ROM downloader, but the reset itself cannot be avoided from user space. The Arduino IDE behaves the same. The serial monitor says so when it opens such a port.
- **device.list cannot tell S3, C3 and C6 apart** from the USB IDs alone (all are 303a:1001). `board.inspect` reads the chip from the ROM.

## Not yet tried on real hardware

- `board.flash`, `board.inspect` and `board.erase`: tested only against QEMU in download mode, which runs the ROM loader without espflash's stub. Erase needs the stub and has never run.
- The reset sequences into the bootloader (USB-Serial-JTAG and DTR/RTS) and back into the app.
- The serial monitor's Reset board button.
