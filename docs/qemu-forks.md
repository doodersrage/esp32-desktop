# QEMU fork comparison (phase 0)

This note records how each QEMU fork runs real firmware, to settle the fork choice in `PLAN.md`. Both forks have been tested as separate processes, and lcgamboa's GPIO and I2C hooks through its library build. The pass/fail checklist is in `PHASE0.md`; this note has the details.

Last updated: 2026-11-15

## Test setup

- **Driver:** `esp32bench sim boot-test` from `core/`. QEMU starts paused, UART0 and QMP connect over Unix sockets, then the CPU runs.
- **Firmware:** the esp32bench.dev release images in `esp32-bench/dist/firmware/`. Both are Arduino builds on ESP-IDF v4.4.7, merged images in DIO mode declaring 4 MiB flash.
  - `pin-detective` 0.1.0: reads GPIO levels and prints them. No Wi-Fi.
  - `probeharbor-probe` 1.5.2: starts Wi-Fi at boot.
- **Wi-Fi firmware:** `tests/firmware/wifi-sta` (IDF v6.0.2) joins the open network `PICSimLabWifi` and prints `esp32bench-wifi got ip <ip>`.
- **Pinned firmware:** `tests/firmware/hello`, built with ESP-IDF v6.0.2 (`tests/firmware/IDF_VERSION`) by `tests/firmware/build.sh`. It prints `esp32bench-hello ready` and then a tick every second. These images are what `cargo test` boots by default.

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

These exist only when QEMU is built as a library (`build_libqemu-esp32.sh`, which leaves `build/libqemu-xtensa.so` and `build/libqemu-riscv32.so`; the ROMs stay in the source tree's `pc-bios/`, so pass `-L <fork>/pc-bios`). `hw/xtensa/esp32_picsimlab.c` (and the C3 equivalent) export:

- `qemu_picsimlab_register_callbacks(callbacks)`: the host supplies function pointers for pin writes and directions (`picsimlab_write_pin`, `picsimlab_dir_pin`), I2C and SPI events, UART TX and RMT.
- `qemu_picsimlab_set_pin(pin, value)` and `qemu_picsimlab_set_apin(channel, value)`: drive digital and analog inputs.
- `qemu_picsimlab_get_internals(cfg)`: read peripheral state such as LEDC duty and UART baud rate.
- `qemu_picsimlab_uart_receive(id, buf, size)` and `qemu_picsimlab_flash_dump(...)`.

The state is one global per process (`global_s`), so one library instance runs one chip. To keep the core's separate-process model, each session would need a small helper process that loads `libqemu-xtensa.so` or `libqemu-riscv32.so`, registers these callbacks and forwards them over a socket. In the plain executables the callbacks are no-ops.

**Tested:** `core/crates/qemu-host` (`esp32bench-qemu-host`) is that helper. The core's session code starts it in place of `qemu-system-*` with the same arguments; it loads the library, registers the callbacks and serves pin events on a Unix socket. With `tests/firmware/gpio` on `esp32-picsimlab` and `esp32c3-picsimlab`, the host saw GPIO 2 toggle every 500 ms and the firmware saw GPIO 4 follow `set 4 1` and `set 4 0` from the host. `core/crates/qemu-host/tests/hooks.rs` repeats this.

With `tests/firmware/i2c-scan` and `ESP32BENCH_I2C_DEVICES=0:3c`, the firmware's scan of bus 0 found only 0x3C, and reading registers 0x10 and 0x11 returned the host's values (0x50, 0x51), on both machines. The same test file covers this.

Details that matter for phase 4:

- The pin map is read during machine set-up, so the set of bridged pins is fixed per session.
- Output and direction changes are reported only for GPIO 0–31. ESP32's GPIO 32 and 33 can be driven as inputs but not watched as outputs.
- `qemu_picsimlab_set_pin` does not take QEMU's big lock (the fork has the call commented out), so the host takes it (`bql_lock_impl`/`bql_unlock`) around each call.
- Callbacks run on QEMU's threads and must not block; the host queues events to a writer thread.
- Library builds use `--enable-debug`, as the fork's script sets it.
- The I2C callback is synchronous: it runs on QEMU's thread and returns the ACK or the byte read. Part models therefore run inside the host process (or behind a blocking round trip), not as asynchronous messages to the core.
- One catch-all device per bus answers every address, so the host decides for the whole bus. On ESP32 bus 0, NACKing 0x48 hides the fork's built-in TMP105.
- In the plain executables the I2C callback is a no-op that ACKs everything, so a firmware scan finds all 112 addresses and reads return 0. Use the library build whenever a project uses I2C.

### Building it in the app (2026-11-16)

`esp32bench toolchain install qemu-lcgamboa` (the setup dialog's Install button) builds the pinned commit without root into `~/.local/share/esp32bench/qemu/lcgamboa-885a6e7061/`: a shallow fetch of the commit, the executables out of tree in `src/build-exe/`, then the fork's `build_libqemu-esp32.sh` for the libraries in `src/build/`. An `installed` marker naming the commit is written last; the build runs in a staging directory, and a failed or cancelled build removes it. About 1.9 GB on disk, 90 seconds on a 16-thread machine.

- Before building it checks for git, make, gcc, pkg-config, ninja, python, perl, glib2, pixman, libgcrypt and libslirp, and shows the `pacman` command for anything missing.
- The out-of-tree executables do not find the ESP32 ROMs by themselves: pass `-L src/pc-bios` (`Build::qemu_args`).
- Machines: `esp32-picsimlab`, `esp32s3`, `esp32c3-picsimlab` (`lcgamboa::machine`). The hello firmware boots and survives reset on all three (`core/crates/sim/tests/boot.rs`), and the GPIO and I2C hooks tests pass against the installed library.

### The simulator on the fork (2026-11-16)

Simulations run on the fork once it is built (`core/crates/sim/src/engine.rs`), else on Espressif's QEMU:

- ESP32 and C3 run through `esp32bench-qemu-host` with the library, with no pins bridged and no socket. The I2C buses then answer no address, as on a board with nothing attached: the `i2c-scan` firmware finds nothing (`hooks.rs`, `*_empty_bus_scans_empty`). For comparison, the fork's plain executables ACK every address, and Espressif's QEMU shows a TMP105 at 0x48 on the ESP32.
- The S3 runs on the fork's executable (`esp32s3` machine); its scan also finds nothing.
- Every session gets `-nic none`. Without any `-nic`, the `wifi-sta` firmware already failed to connect on both forks, but the default is now explicit; a network needs a consent decision first (open question below).
- Console, reset and GDB (breakpoints, stepping, registers) pass on all three chips on both engines (`core/crates/debug/tests/qemu.rs`, the daemon's `sim_*` tests).

### The virtual display (2026-11-16)

Both forks have Espressif's virtual RGB panel (`hw/display/esp_rgb.c`), which firmware draws on with the `esp_lcd_qemu_rgb` component (pinned at 1.0.2 in `tests/firmware/rgb`). Findings:

- **The panel needs a display refresh.** The driver's draw call spins until the device marks the update done, and the device does that only when a display refreshes it. With `-display none` nothing does, so the first draw hangs and the task watchdog fires. QMP `screendump` refreshes it, so the core polls every session with `screendump` (every 40 ms while the picture changes, 100 ms otherwise; about 1 ms each at 320x240). The same poll gives the picture.
- **An unconfigured panel is 800x600 in one colour.** Frames are reported only once the picture differs from that, so firmware without a display shows nothing.
- **The fork's library build had no display:** its script disables pixman, so QEMU stubs out display surfaces and has no `screendump`. Patch 0001 enables it.
- **`esp32-picsimlab` did not read "QEMU"** at APB_CTRL + 0x78, which the driver checks, so it refused the panel ("not available on real hardware"). The fork's `esp32` machine sets it by adding the date register's memory region a second time, which is the assertion that makes that machine abort. Patch 0002 gives the register its own region.
- With both patches the `rgb` test firmware draws and updates on all three chips on both forks (`core/crates/sim/tests/display.rs`).

### Arduino builds panic on the fork (found 2026-11-16, not fixed)

Arduino core 3.3.12 builds (Blink, the library's `oled`) panic with `Guru Meditation Error ... (Cache error). Cache disabled but cached memory region accessed` on lcgamboa's fork, usually within seconds of start-up, in about half of runs. They then reboot, and often come up on a later try. Espressif's QEMU runs the same images cleanly. ESP-IDF test firmware does not trigger it, which is why the automated tests never saw it.

- Where: core 0 is in `nvs_flash_init` (Arduino's `initArduino`), reading flash; it has disabled both cores' caches, and the trap fires as it re-enables them in `spi_flash_restore_cache`. Core 1 is parked in `spi_flash_op_block_func`, as it should be. ESP-IDF firmware that touches flash after boot would hit it too.
- Not the cause, tested: the fork's changes to `esp32_cache_data_sync` (swapping in Espressif's version changed nothing); the per-CPU cache wiring (identical to Espressif's `esp32.c`); multi-threaded TCG (`-accel tcg,thread=single` crashed more, not less). It happens on the fork's executable as well as its library.
- Upstream: our pin is still the fork's head, and no issue mentions it.

Left to compare with Espressif's tree: interrupt routing and masking during flash operations (`esp32_intc.c`, the interrupt matrix), and the DPORT cache control and trap-enable registers.

## Open questions

- Should simulator boot tests stub out Wi-Fi, or should a project be marked as "no simulator" when it starts Wi-Fi at boot? lcgamboa's fork answers this for ESP32 and C3 only.
- Emulated networking gives guest firmware network access. The app needs a consent decision before enabling it (rule 4); `restrict=on` is a safe default.
