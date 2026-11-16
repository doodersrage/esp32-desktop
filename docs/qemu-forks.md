# QEMU fork comparison (phase 0)

This note records how each QEMU fork runs real firmware, to settle the fork choice in `PLAN.md`. The Espressif fork has been tested; the lcgamboa fork has not.

Last updated: 2026-11-15

## Test setup

- **Driver:** `esp32d sim boot-test` from `core/`. QEMU starts paused, UART0 and QMP connect over Unix sockets, then the CPU runs.
- **Firmware:** the esp32bench.dev release images in `esp32-bench/dist/firmware/`. Both are Arduino builds on ESP-IDF v4.4.7, merged images in DIO mode declaring 4 MiB flash.
  - `pin-detective` 0.1.0: reads GPIO levels and prints them. No Wi-Fi.
  - `probeharbor-probe` 1.5.2: starts Wi-Fi at boot.
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

## lcgamboa fork

Not tested yet. Planned checks:

- Same two images on all three chips, especially whether probeharbor-probe gets past Wi-Fi start.
- Whether a current IDF build boots (Velxio's docs pin it to QEMU 8.1.3 and IDF 4.4.x).
- GPIO, ADC, UART and I2C callbacks from the host, using pin-detective to read levels driven from outside.
- How it is built and loaded: as a library, which the core cannot drive as a separate process the same way.

## Open questions

- Should simulator boot tests stub out Wi-Fi, or should a project be marked as "no simulator" when it starts Wi-Fi at boot?
