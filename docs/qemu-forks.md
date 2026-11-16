# QEMU fork comparison (phase 0)

This note records how each QEMU fork runs real firmware, to settle the fork choice in `PLAN.md`. Both forks have been tested as separate processes; lcgamboa's library mode (GPIO and I2C hooks) has not. The pass/fail checklist is in `PHASE0.md`; this note has the details.

Last updated: 2026-11-15

## Test setup

- **Driver:** `esp32d sim boot-test` from `core/`. QEMU starts paused, UART0 and QMP connect over Unix sockets, then the CPU runs.
- **Firmware:** the esp32bench.dev release images in `esp32-bench/dist/firmware/`. Both are Arduino builds on ESP-IDF v4.4.7, merged images in DIO mode declaring 4 MiB flash.
  - `pin-detective` 0.1.0: reads GPIO levels and prints them. No Wi-Fi.
  - `probeharbor-probe` 1.5.2: starts Wi-Fi at boot.
- **Wi-Fi firmware:** `tests/firmware/wifi-sta` (IDF v6.0.2) joins the open network `PICSimLabWifi` and prints `esp32d-wifi got ip <ip>`.
- **Pinned firmware:** `tests/firmware/hello`, built with ESP-IDF v6.0.2 (`tests/firmware/IDF_VERSION`) by `tests/firmware/build.sh`. It prints `esp32d-hello ready` and then a tick every second. These images are what `cargo test` boots by default.

## Espressif fork, `esp-develop-9.2.2-20261109`

Prebuilt x86_64 binaries from GitHub, run on Arch with the official-repo runtime dependencies. No extra QEMU options were needed: no watchdog override and no `-icount`. The hello firmware ran 75 seconds on each chip without a watchdog reset, but it is idle between ticks; busier firmware may still trip the watchdog.

| Chip | hello (IDF v6.0.2) | pin-detective (IDF v4.4.7) | probeharbor-probe (IDF v4.4.7) |
| --- | --- | --- | --- |
| ESP32 | Boots, runs 75 s, survives QMP reset | Boots to app, survives QMP reset | Panics when Wi-Fi starts, then boot-loops |
| ESP32-S3 | Boots, runs 75 s, survives QMP reset | Boots to app, survives QMP reset | Prints its banner, then hangs (Wi-Fi start presumed) |
| ESP32-C3 | Boots, runs 75 s, survives QMP reset | Boots to app, survives QMP reset | Prints its banner, then hangs (Wi-Fi start presumed) |

Findings:

1. **Control channels work on all three chips.** UART0 over a socket chardev, QMP `cont`, `system_reset` and `quit`, and the GDB stub (`-gdb tcp:127.0.0.1:<port>`, stop reply `T05`) all work.
2. **No Wi-Fi radio.** On ESP32 the firmware faults with `LoadStorePIFAddrError` reading `0x60033c00`, in the Wi-Fi MAC register range. Any library project that starts Wi-Fi at boot will not run on this fork without changes. This confirms the Wi-Fi gap that `PLAN.md` had listed as unverified.
3. **Blank eFuses.** The base MAC reads `00:00:00:00:00:00` on every chip. Firmware that derives an ID from the MAC sees all zeros.
4. **CPU clock reports differ.** pin-detective reports 240 MHz on ESP32 but 80 MHz on S3 and C3. Not investigated.
5. **GPIO:** pin-detective reads every pin as low. Nothing in this fork lets the host drive or read pins; that was not tested further.
6. **Current IDF works.** IDF v6.0.2 firmware boots on all three chips. S3 logs `W rtcinit: o_code calibration fail` at boot, which is harmless.

### Options `idf.py qemu` adds that the core does not

From `tools/idf_py_actions/qemu_ext.py` in IDF v6.0.2. None was needed for the tests above.

| Option | What it does | Status in the core |
| --- | --- | --- |
| `-global driver=timer.<chip>.timg,property=wdt_disable,value=true` | Turns off the timer-group watchdog | Not added; add it if busy firmware trips it |
| `-m 4M` (ESP32), `-m 32M` (S3) | PSRAM size | Not added; needed for PSRAM projects |
| eFuse image on `nvram.<chip>.efuse` | Sets chip revision 3 (ESP32) or 0.3 (S3, C3) and gives a MAC | Not added; explains the all-zero MAC |
| `-nic user,model=open_eth` | Emulated Ethernet with user-mode networking | Deliberately left out: it gives untrusted firmware network access, which needs a consent decision first (rule 4) |
| `-global driver=<chip>.gpio,property=strap_mode,...` | Boots into download mode | Not needed; only for flashing over emulated UART |

## lcgamboa fork, commit `885a6e7061` (2026-11-14)

Built from source at `~/esp/qemu-lcgamboa` as ordinary executables (`./configure --target-list=xtensa-softmmu,riscv32-softmmu --enable-slirp --enable-gcrypt`), with no patches. The fork has no releases or tags, so the pin is a commit. It is QEMU 9.2.2, the same base as the Espressif build above. Licence: GPLv2, as QEMU; GitHub detects none.

Machines: `esp32`, `esp32-picsimlab`, `esp32s3`, `esp32c3` and `esp32c3-picsimlab`. There is no S3 PICSimLab machine.

| Chip and machine | hello (IDF v6.0.2) | pin-detective (IDF v4.4.7) | wifi-sta (IDF v6.0.2) |
| --- | --- | --- | --- |
| ESP32, `esp32` | QEMU aborts at start-up | QEMU aborts at start-up | QEMU aborts at start-up |
| ESP32, `esp32-picsimlab` | Boots, survives QMP reset | Boots, survives QMP reset | Gets IP 192.168.4.15 |
| ESP32-S3, `esp32s3` | Boots, survives QMP reset | Flash init fails, boot-loops | No Wi-Fi model |
| ESP32-C3, `esp32c3` | Boots, survives QMP reset | Flash init fails, boot-loops | not tried |
| ESP32-C3, `esp32c3-picsimlab` | Boots, survives QMP reset | Flash init fails, boot-loops | Gets IP 192.168.4.15 with an eFuse image; aborts without one |

Findings:

1. **The plain `esp32` machine is broken at this commit.** It aborts during machine set-up with `memory_region_add_subregion_common: Assertion '!subregion->container' failed`, even with no flash attached. `esp32-picsimlab` works.
2. **Wi-Fi works on ESP32 and C3.** Pass `-nic user,model=esp32_wifi` (or `esp32c3_wifi`); the station joins one of the hard-coded open SSIDs and gets 192.168.4.15. probeharbor-probe, which panics on the Espressif fork, starts its setup access point here. Tests used `restrict=on` so the guest could not reach the host network.
3. **C3 Wi-Fi needs an eFuse image.** Without one QEMU aborts with `esp_efuse_reload_from_blk: Assertion 's->mirror' failed`. The default image `idf.py qemu` writes (1024 bytes, chip revision 0.3) works.
4. **Older IDF fails on S3 and C3.** IDF 4.4.7 builds (Arduino core 2.x) stop with `assert failed: do_core_init startup.c:328 (flash_ret == ESP_OK)` and reboot. IDF v6.0.2 builds are fine, so the fork's S3/C3 flash model only matches newer IDF.
5. **UART input, QMP reset, GDB and timing match the Espressif fork.** Results are in `PHASE0.md`.

### GPIO, I2C and other host hooks

These exist only when QEMU is built as a library. `hw/xtensa/esp32_picsimlab.c` (and the C3 equivalent) export:

- `qemu_picsimlab_register_callbacks(callbacks)`: the host supplies function pointers for pin writes and directions (`picsimlab_write_pin`, `picsimlab_dir_pin`), I2C and SPI events, UART TX and RMT.
- `qemu_picsimlab_set_pin(pin, value)` and `qemu_picsimlab_set_apin(channel, value)`: drive digital and analog inputs.
- `qemu_picsimlab_get_internals(cfg)`: read peripheral state such as LEDC duty and UART baud rate.
- `qemu_picsimlab_uart_receive(id, buf, size)` and `qemu_picsimlab_flash_dump(...)`.

The state is one global per process (`global_s`), so one library instance runs one chip. To keep the core's separate-process model, each session would need a small helper process that loads `libqemu-xtensa.so` or `libqemu-riscv32.so`, registers these callbacks and forwards them over a socket. In the plain executables the callbacks are no-ops.

## Open questions

- Should simulator boot tests stub out Wi-Fi, or should a project be marked as "no simulator" when it starts Wi-Fi at boot? lcgamboa's fork answers this for ESP32 and C3 only.
- Do the lcgamboa library hooks work from a helper process? Untested; it needs a library build and a small host program.
- Emulated networking gives guest firmware network access. The app needs a consent decision before enabling it (rule 4); `restrict=on` is a safe default.
