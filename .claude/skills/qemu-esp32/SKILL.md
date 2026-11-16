---
name: qemu-esp32
description: Run or debug ESP32 firmware in QEMU for this project. Use when booting firmware in the simulator, wiring up UART, QMP or GDB to QEMU, writing a QEMU boot test, or comparing the Espressif and lcgamboa QEMU forks.
---

# Running ESP32 firmware in QEMU

This project drives QEMU as a child process. The simulator uses lcgamboa's fork (decided in phase 0); Espressif's prebuilt build is still what `esp32bench qemu install` fetches, as a baseline. Everything below was confirmed in phase 0 unless it says otherwise.

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

For Arduino builds, ESP32 Arduino core 3.x already writes `<sketch>.ino.merged.bin`, padded to the flash size (see `tests/firmware/build.sh`). Build with `FlashMode=dio`.

## 4. Launch

The core does this for you: `esp32bench sim run --chip <chip> <image>` or `esp32bench sim boot-test`. It uses the pinned build from `esp32bench qemu install`, starts QEMU paused with `-display none -monitor none -S`, connects UART0 and QMP, then sends `cont`. Do not add `-no-reboot`: it turns a QMP `system_reset` into a shutdown. Verified with `esp-develop-9.2.2-20261109` on ESP32, S3 and C3.

The equivalent by hand:

```sh
qemu-system-xtensa \
  -nographic \
  -machine esp32 \
  -drive file=flash.bin,if=mtd,format=raw \
  -serial unix:/run/user/$UID/esp32bench/uart0.sock,server,nowait \
  -qmp unix:/run/user/$UID/esp32bench/qmp.sock,server,nowait
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

Gotchas found building the debugger (`core/crates/debug`):

- Keep UART0 drained. If nothing reads the UART socket, QEMU stalls once its buffer fills, and a `continue` never reaches the breakpoint.
- Set `mi-async on` before connecting, or GDB will not take `-exec-interrupt` while the target runs. Breakpoints still need the target stopped.
- Use hardware breakpoints (`-break-insert -h`): code runs from flash.
- On the C3, `-data-list-register-values` for all registers fails (QEMU cannot read the `htimedelta` CSR). Ask for registers by number.
- Start GDB with `-iex "set auto-load off"` before the ELF: a downloaded project's ELF could carry GDB scripts.

## 6. Boot tests

A boot test passes when an expected marker string appears on UART0 within a timeout, and fails on a panic banner or timeout. Always kill QEMU at the end of the test, pass or fail.

In `core/crates/sim/tests/boot.rs` the tests skip unless QEMU is installed and `ESP32BENCH_TEST_IMAGE_<CHIP>` (e.g. `ESP32BENCH_TEST_IMAGE_ESP32C3`) points at a merged image. `rst:0x` only proves the ROM ran; use an app-level marker to prove the firmware booted.

Known gap in Espressif's fork: no Wi-Fi radio. Firmware that starts Wi-Fi panics on ESP32 (`LoadStorePIFAddrError` at `0x60033c00`) and hangs on S3 and C3. Results are in `docs/qemu-forks.md`.

## Fork comparison (phase 0)

Run the same firmware against both forks and record results in `docs/`:

- Espressif fork: official, current with IDF, limited peripheral hooks.
- lcgamboa fork: builds as a library, adds Wi-Fi, exposes GPIO, ADC, UART and I2C callbacks, but may lag current IDF versions.

Record for each: which IDF and Arduino core versions boot, which peripherals respond, and how GPIO state can be read and driven from the host.

Results so far are in `docs/PHASE0.md` (checklist) and `docs/qemu-forks.md` (details). Known traps with lcgamboa's fork at commit `885a6e7061`:

- Use `-machine esp32-picsimlab`; the plain `esp32` machine aborts at start-up.
- Wi-Fi: `--qemu-arg=-nic --qemu-arg=user,model=esp32_wifi,restrict=on` (or `esp32c3_wifi`). C3 Wi-Fi also needs an eFuse image on `nvram.esp32c3.efuse`, or QEMU aborts.
- IDF 4.4 builds fail flash init on S3 and C3; IDF 6 builds boot.
- GPIO, I2C and SPI hooks are C callbacks in library builds only.

To run a build other than the pinned one: `esp32bench sim boot-test --qemu <binary> [--machine <name>] [--qemu-arg=<arg>]...`.
