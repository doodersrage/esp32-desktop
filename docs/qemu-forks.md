# QEMU fork comparison (phase 0)

This note records how each QEMU fork runs real firmware, to settle the fork choice in `PLAN.md`. The Espressif fork has been tested; the lcgamboa fork has not.

Last updated: 2026-11-15

## Test setup

- **Driver:** `esp32d sim boot-test` from `core/`. QEMU starts paused, UART0 and QMP connect over Unix sockets, then the CPU runs.
- **Firmware:** the esp32bench.dev release images in `esp32-bench/dist/firmware/`. Both are Arduino builds on ESP-IDF v4.4.7, merged images in DIO mode declaring 4 MiB flash.
  - `pin-detective` 0.1.0: reads GPIO levels and prints them. No Wi-Fi.
  - `probeharbor-probe` 1.5.2: starts Wi-Fi at boot.
- **Not yet tested:** a current IDF (v6.x) build. `tests/firmware/` has no pinned hello-world yet because ESP-IDF is not installed on the test machine.

## Espressif fork, `esp-develop-9.2.2-20261109`

Prebuilt x86_64 binaries from GitHub, run on Arch with the official-repo runtime dependencies. Runs of up to 15 seconds needed no extra QEMU options: no watchdog override and no `-icount`. Longer runs have not been tried.

| Chip | pin-detective | probeharbor-probe |
| --- | --- | --- |
| ESP32 | Boots to app, survives QMP reset | Panics when Wi-Fi starts, then boot-loops |
| ESP32-S3 | Boots to app, survives QMP reset | Prints its banner, then hangs (Wi-Fi start presumed) |
| ESP32-C3 | Boots to app, survives QMP reset | Prints its banner, then hangs (Wi-Fi start presumed) |

Findings:

1. **Control channels work on all three chips.** UART0 over a socket chardev, QMP `cont`, `system_reset` and `quit`, and the GDB stub (`-gdb tcp:127.0.0.1:<port>`, stop reply `T05`) all work.
2. **No Wi-Fi radio.** On ESP32 the firmware faults with `LoadStorePIFAddrError` reading `0x60033c00`, in the Wi-Fi MAC register range. Any library project that starts Wi-Fi at boot will not run on this fork without changes. This confirms the Wi-Fi gap that `PLAN.md` lists as unverified.
3. **Blank eFuses.** The base MAC reads `00:00:00:00:00:00` on every chip. Firmware that derives an ID from the MAC sees all zeros.
4. **CPU clock reports differ.** pin-detective reports 240 MHz on ESP32 but 80 MHz on S3 and C3. Not investigated.
5. **GPIO:** pin-detective reads every pin as low. Nothing in this fork lets the host drive or read pins; that was not tested further.

## lcgamboa fork

Not tested yet. Planned checks:

- Same two images on all three chips, especially whether probeharbor-probe gets past Wi-Fi start.
- Whether a current IDF build boots (Velxio's docs pin it to QEMU 8.1.3 and IDF 4.4.x).
- GPIO, ADC, UART and I2C callbacks from the host, using pin-detective to read levels driven from outside.
- How it is built and loaded: as a library, which the core cannot drive as a separate process the same way.

## Open questions

- Does a current IDF hello-world boot on the Espressif fork? Needs ESP-IDF installed and a project in `tests/firmware/`.
- Should simulator boot tests stub out Wi-Fi, or should a project be marked as "no simulator" when it starts Wi-Fi at boot?
