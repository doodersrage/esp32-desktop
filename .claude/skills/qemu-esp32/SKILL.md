---
name: qemu-esp32
description: Run or debug ESP32 firmware in QEMU for this project. Use when booting firmware in the simulator, wiring up UART, QMP or GDB to QEMU, writing a QEMU boot test, or comparing the Espressif and lcgamboa QEMU forks.
---

# Running ESP32 firmware in QEMU

This project drives QEMU as a child process. The commands below are the starting point for phase 0. Anything marked "verify" has not yet been confirmed against the pinned QEMU build; confirm it and update this file when you do.

## 1. Get QEMU

Espressif publishes prebuilt binaries through ESP-IDF:

```sh
python "$IDF_PATH/tools/idf_tools.py" install qemu-xtensa qemu-riscv32
. "$IDF_PATH/export.sh"
```

Arch runtime dependencies: `libgcrypt glib2 pixman sdl2 libslirp`.

Do not use the AUR package `qemu-esp-xtensa-git`. It is stale.

## 2. Pick the machine

| Chip | Binary | Machine |
| --- | --- | --- |
| ESP32 | `qemu-system-xtensa` | `esp32` |
| ESP32-S3 | `qemu-system-xtensa` | `esp32s3` |
| ESP32-C3 | `qemu-system-riscv32` | `esp32c3` |

Other chips (S2, C6) are not simulated. Do not add them to the simulator.

## 3. Build a flash image

QEMU needs one merged image whose size is exactly 2, 4, 8 or 16 MB, containing the bootloader, partition table and app at their offsets.

For an ESP-IDF project, from the build directory:

```sh
esptool.py --chip esp32 merge_bin --fill-flash-size 4MB -o flash.bin @flash_args
```

Flash mode must be DIO or DOUT. QIO and QOUT do not boot in QEMU.

For Arduino builds, merge the bootloader, partition and app binaries that `arduino-cli compile --output-dir` produces, at the offsets for that chip (verify the offsets per chip).

## 4. Launch

The core does this for you: `esp32d sim run --chip <chip> <image>` or `esp32d sim boot-test`. It uses the pinned build from `esp32d qemu install`, starts QEMU paused with `-display none -monitor none -S`, connects UART0 and QMP, then sends `cont`. Do not add `-no-reboot`: it turns a QMP `system_reset` into a shutdown. Verified with `esp-develop-9.2.2-20261109` on ESP32, S3 and C3.

The equivalent by hand:

```sh
qemu-system-xtensa \
  -nographic \
  -machine esp32 \
  -drive file=flash.bin,if=mtd,format=raw \
  -serial unix:/run/user/$UID/esp32-desktop/uart0.sock,server,nowait \
  -qmp unix:/run/user/$UID/esp32-desktop/qmp.sock,server,nowait
```

- UART0 is the `-serial` socket. Treat its output as bytes.
- QMP is the control channel. Send `{"execute":"qmp_capabilities"}` first, then commands such as `system_reset`, `stop`, `cont` and `quit`.
- Add `-s -S` to open a GDB server on TCP port 1234 and wait for a debugger. For a per-session port use `-gdb tcp::<port>`.

## 5. Debug

Attach the toolchain GDB that matches the chip (`xtensa-esp32-elf-gdb`, `xtensa-esp32s3-elf-gdb` or `riscv32-esp-elf-gdb`) with the project's ELF:

```
target remote :1234
```

The core speaks GDB/MI to it; the same client is used for real boards.

## 6. Boot tests

A boot test passes when an expected marker string appears on UART0 within a timeout, and fails on a panic banner or timeout. Always kill QEMU at the end of the test, pass or fail.

In `core/crates/sim/tests/boot.rs` the tests skip unless QEMU is installed and `ESP32D_TEST_IMAGE_<CHIP>` (e.g. `ESP32D_TEST_IMAGE_ESP32C3`) points at a merged image. `rst:0x` only proves the ROM ran; use an app-level marker to prove the firmware booted.

Known gap in Espressif's fork: no Wi-Fi radio. Firmware that starts Wi-Fi panics on ESP32 (`LoadStorePIFAddrError` at `0x60033c00`) and hangs on S3 and C3. Results are in `docs/qemu-forks.md`.

## Fork comparison (phase 0)

Run the same firmware against both forks and record results in `docs/`:

- Espressif fork: official, current with IDF, limited peripheral hooks.
- lcgamboa fork: builds as a library, adds Wi-Fi, exposes GPIO, ADC, UART and I2C callbacks, but may lag current IDF versions.

Record for each: which IDF and Arduino core versions boot, which peripherals respond, and how GPIO state can be read and driven from the host.
