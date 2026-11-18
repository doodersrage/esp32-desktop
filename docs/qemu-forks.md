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

### Arduino builds panicked on the fork (found and fixed 2026-11-16)

Arduino core 3.3.12 builds (Blink, the library's `oled`) panicked within seconds on lcgamboa's fork in about half of runs, as `Guru Meditation Error ... (Cache error)`, and then rebooted. Espressif's QEMU ran them cleanly; ESP-IDF test firmware never showed it.

- **Cause:** a CPU clock change. Arduino switches the CPU to 240 MHz at run time (`setCpuFrequencyMhz` in `initArduino`), with the tick timers armed; ESP-IDF firmware switches during early boot. The `esp32-picsimlab` machine set each CPU's input clock directly, and the fork's CPU had no clock-change callback, so CCOUNT was rescaled and a pending CCOMPARE deadline could land a whole counter wrap away (~18 s at 240 MHz). Seen with GDB at the panic: core 0's CCOUNT ~235M cycles past CCOMPARE0, no timer interrupt pending; its tick had stopped. A stopped tick also starves the interrupt watchdog; which panic appears first depends on the interrupt matrix.
- **Fix:** patch 0003 (`core/crates/sim/patches/0003-cpu-clock-changes.patch`) takes Espressif's `xtensa_cpu_clock_changed()` and drives both CPUs from one `cpu-clk` source clock, as their `esp32.c` does (a clock's callback only runs when its source propagates).
- **Evidence:** Blink panicked in 6 to 9 of 12 runs before, and 16 of 16 ran clean after; the `oled` sketch 6 of 6 with its display part. `core/crates/sim/tests/boot.rs` (`lcgamboa_*_runs_arduino`) fails within seconds without the patch.
- **Ruled out on the way:** the fork's cache refill changes, its per-CPU cache wiring, multi-threaded TCG, and swapping in Espressif's interrupt matrix and timer group (the matrix turned the panic into an interrupt-watchdog timeout, which led to the cause; neither is needed). `tests/firmware/flashop` (flash reads on one core while the other runs) came out of this too; it runs clean on both forks.
- **Upstream:** our pin is the fork's head; this is worth reporting to lcgamboa.

### Wi-Fi firmware needs a radio (found 2026-11-16)

With `-nic none`, Wi-Fi start-up on the fork's ESP32 crashes (`LoadStorePIFAddrError` in `esp_phy_enable` / `register_chipv7_phy`): the radio model exists only with a NIC. ESP32 sessions on the fork therefore get `-nic hubport,hubid=0,model=esp32_wifi`: a radio on an empty hub, with no DHCP, DNS or route anywhere. `-nic user,restrict=on` was considered and rejected: it isolates the guest from the host and the outside, but QEMU's user-mode stack still forwards DNS queries to the host's resolver. `wifi-sta` now starts, gets no address and keeps running (`lcgamboa_esp32_wifi_has_a_radio_but_no_network`). The C3's radio (`esp32c3_wifi`) reads its MAC address from the eFuse device while the machine is still being built, before that device is realized; without an eFuse file, the device's defaults are only set at realize, so QEMU aborts (`assert(s->mirror != NULL)`). With a file it works, but a blank file fails the bootloader's chip-revision check ("Required >= v0.3, found v0.0"). C3 sessions therefore get a 1024-byte image (`lcgamboa::esp32c3_efuse`) holding what the fork's defaults would set: chip revision v0.3, eFuse block revision v1.3, MAC c4:0a:24:fe:10:01. `wifi-sta` then starts, gets no address and keeps running on the C3 too (`lcgamboa_esp32c3_wifi_has_a_radio_but_no_network`). Upstream could fix it by realizing the eFuses before the NIC.

### Library sketches that hung or aborted: ADC and LEDC (found and fixed 2026-11-18)

The first library sweep (`docs/library-sweep.md`) booted every ESP32, S3 and C3 build. Two register-level gaps stopped sketches that do nothing unusual:

- **S3 SAR ADC.** Any Arduino sketch that links the ADC driver runs `adc_hw_calibration()` before `setup()`. It starts a one-shot conversion and polls `SENS_SAR_MEAS1_CTRL2_REG`'s `MEAS1_DONE_SAR` (0x6000880C, bit 16). The `esp32s3` machine leaves the SENS range unimplemented, so the bit never sets and the sketch hangs silently, on Espressif's QEMU too. Found with GDB attached to the hung `talking-plant` build (`adc_oneshot_ll_get_event` in `read_cal_channel`).
- **LEDC readback, C3 and S3.** ESP-IDF's LEDC driver reads back `LEDC_CONF_REG`'s clock source (`ledc_get_freq()` aborts on 0) and each channel's `DUTY_R`. The C3 model returned only timer and channel CONF0 registers; the S3 has no LEDC model. So `tone()` aborted (`simon`, `morse-decoder`, `keypad-lock`). Found by decoding the abort's backtrace with the core's `addr2line`.
- **Fix:** patch 0004 gives the S3 SENS and LEDC ranges as registers that keep what is written, a conversion that completes at once and reads 0 (analog inputs are not simulated), and `DUTY_R` mirroring `DUTY`. Patch 0005 makes the C3's LEDC registers read back the same way, keeping its duty reporting. Recipe 5.
- **Evidence:** `tests/firmware/arduino-periph` (analogRead, tone, analogWrite) completes on all three chips with the patches (`lcgamboa_*_runs_adc_ledc_and_rmt` in `core/crates/sim/tests/boot.rs`); without them the C3 and S3 tests fail.

### The C3's USB serial console (added 2026-11-18)

Boards without a USB-UART chip, like the C3 SuperMini, put Arduino's `Serial` on the C3's USB-Serial-JTAG (USB CDC on boot). Both forks model that peripheral as an empty stub, so 41 library builds ran silently. Patch 0006 models its serial side: EP1 writes go to the device's chardev on `WR_DONE` or when the FIFO fills (raising `SERIAL_IN_EMPTY`), chardev input arrives on EP1 with `SERIAL_OUT_RECV_PKT`, the interrupt is wired to the matrix, and `FRAM_NUM` advances with virtual time like a host's start-of-frame, which ESP-IDF's connection monitor and Arduino's `HWCDC` check before sending. A session that wants it passes `-chardev socket,id=esp32bench-usb,...` and `-global driver=misc.esp32c3.usb_serial_jtag,property=chardev,value=esp32bench-usb`, and the core merges that socket into the console (`esp32bench-sim::uart`), sending input to both ports.

- **A trap on the way:** QEMU never accepted the device's socket. The fork's main loop runs its own GLib context, `g_main_context_default_l`, not the default one, and devices must register chardev handlers on it, as the fork's UARTs do. Found by tracing `tcp_chr_accept`.
- **Enabled per board, not for every C3 session:** with the USB console wired, about one ESP-IDF boot in five on the C3 lost its UART0 output partway through start-up (the firmware kept running), in `esp32c3_session_bridges_pins`. Without it, 0 in 20. Not explained: blocking vs non-blocking chardev writes, a never-full IN FIFO, `-serial` vs `-chardev` wiring and waiting for the accept before starting the CPU made no difference. So the daemon turns it on only for boards whose builds put `Serial` on USB (`esp32bench_build::serial_on_usb`: the C3 SuperMini), and `sim run` has `--usb-console`. Those sessions were reliable (25 of 25).
- **Test:** `tests/firmware/hello-arduino` is also built for the C3 with Serial on USB (`hello-arduino-esp32c3-usb.bin`), and `lcgamboa_esp32c3_usb_console_both_ways` sees its output and gets an echo back. Recipe 6.

### WS2812 LEDs: RMT transmission (fixed 2026-11-18)

ESP-IDF 5's RMT driver, which Arduino's `rmtWrite()`, `rgbLedWrite()` and Adafruit_NeoPixel use, never saw its first transmission end, so NeoPixel sketches (and the C3 DevKitM's on-board RGB LED) stopped at the first LED write. Patch 0007, recipe 7:

- **ESP32** (`hw/ssi/esp32_rmt.c`): the model held data back while any threshold or end bit was raised, so the ISR could refill memory first. The driver turns the threshold interrupt off once it has encoded everything, without clearing the bit, and the model then waited forever. Found by tracing the register traffic: after 192 symbols (eight LEDs), `INT_RAW` held the threshold bit with only the end interrupt enabled. Now only an enabled interrupt holds data back, turning it off lets held data go, and the interrupt line follows `INT_RAW & INT_ENA` (it was lowered only by `INT_CLR`, so the CPU sat in the shared interrupt handler). Also fixed on the way: channels other than 0 read channel 0's memory, and a symbol with level bits but zero durations did not end the transmission.
- **C3 and S3**: no RMT model at all (an unimplemented-device stub). `hw/misc/esp32bench_rmt.c` is new: the transmit side the driver uses, with each chip's register layout from ESP-IDF's `rmt_struct.h`. `TX_START`, `CONF_UPDATE` and the reset bits clear themselves (the driver writes the channel register read-modify-write); symbols are read from RMT memory, with the threshold interrupt every `TX_LIM` symbols, the end interrupt at a zero duration, loop mode with its count, and `TX_STOP`. Symbols take the time their durations give at the channel's clock. Nothing is output, and receive channels only keep their registers.
- **Test:** `tests/firmware/arduino-periph` writes 192 symbols from one 48-symbol memory block (so the driver refills it on threshold interrupts) and one `rgbLedWrite()`, on all three chips (`lcgamboa_*_runs_adc_ledc_and_rmt`). The library's `neopixel` and `fire-lamp` run their effects on all three.

### Sleep (added 2026-11-18)

ESP-IDF enters sleep by setting `RTC_CNTL_STATE0`'s `SLEEP_EN`, then spins until the chip powers down (deep sleep) or RTC_CNTL raises `SLP_WAKEUP` (light sleep). No RTC_CNTL model handled it: light sleep never returned, and deep sleep spun until a watchdog reset the chip, with a watchdog's reset reason and RTC memory lost, so the library's battery sketches rebooted every second or so. Patch 0008, recipe 8, models sleep on a timer on all three chips:

- Each RTC_CNTL keeps the registers the sleep code writes and reads back, and on `SLEEP_EN` arms a timer for the wake-up time (RTC slow-clock ticks to virtual time). Light sleep: the timer raises `SLP_WAKEUP`. Deep sleep (`DG_WRAP_PD_EN`): the CPUs are held and the digital domain reset, which stops the app's watchdogs while RTC memory and RTC registers are kept; on the timer the chip boots through the ROM with reset reason `DEEPSLEEP_RESET` and wake-up cause "timer".
- **ESP32:** the CPUs are held through RTC_CNTL's CPU stall lines. The first version released them on the timer and the boot then died to timer group 0's watchdog: the digital reset at sleep entry arms the flash-boot watchdog, as a real reset does, and it counted while the CPUs were held. Waking now resets the digital domain again, as powering it up does, and the SoC releases the CPUs when that reset is done.
- **S3:** the machine's CPU stall input was empty; it now honours the deep-sleep hold only.
- **C3:** a reset at sleep entry, with the CPU held halted (`start_powered_off`) until the timer fires. The C3 model also had no RTC time (it read 0); it now counts.
- **RTC memory CRC:** before deep sleep, ESP-IDF runs the RTC memory CRC unit, and the ROM runs it again after it, to decide whether to run the wake stub (`set_rtc_memory_crc`: RTC_CNTL 0x100/0x104 on the ESP32, `SYSTEM_RTC_FASTMEM_CONFIG`/`CRC` on the C3). Unmodelled, its "finished" bit never set and deep sleep hung before it began. It finishes at once with a fixed result: only the two results are compared, and RTC memory is always kept. Found with GDB in the ROM.
- Only the timer wakes the chip; GPIO (ext0/ext1), touch and ULP wake-ups are not modelled, so a sketch waiting for a button sleeps until reset. Any other reset ends sleep.
- **Test:** `tests/firmware/arduino-sleep` sleeps 200 ms lightly, then deep-sleeps twice on a 500 ms timer, counting boots in RTC memory (`lcgamboa_*_sleeps_and_wakes`): boot 2 and 3 report cause 4 (timer) and reset reason 8 (deep sleep), on all three chips, about 0.5 s apart. The library's `deep-sleep` and `battery-sensor` builds pass.

### DHT sensors and the interrupt watchdog (found 2026-11-18)

`dht22` on the ESP32 panicked with "Cache error" in some sweeps and not in reruns. It is the interrupt watchdog, misreported: the trap the fork raises for cache errors never fired (traced), and the panic printed core "/" (no CPU with a cache error). With no sensor attached, the DHT library spins on `digitalRead()` with interrupts off for up to 240,000 iterations. That takes about 166 ms on a 240 MHz chip. Here virtual time follows the host clock and each GPIO read is an MMIO exit, so it takes about 250 ms on an idle host and over the watchdog's 300 ms on a loaded one (6 of 24 runs at eight sessions at once, 0 of 24 for `hello-arduino`). With `-icount shift=2` (virtual time from instructions executed, about 250 MIPS) the same read takes a steady 160 ms on Espressif's QEMU, as on the chip, whatever the host load. Sessions do not use `-icount`: it needs single-threaded TCG, the fork's library host did not start with it, and boot was slower. Using it would make all timing deterministic; it is an open question.

### The simulated Wi-Fi network (added 2026-11-18)

Sessions on the fork's ESP32 and C3 put QEMU's user-mode network, `restrict=on`, on hub 0 beside the radio (`esp32bench-sim::network`). The fork's open access point `PICSimLabWifi` then hands out 10.0.2.15 by DHCP. `tests/firmware/arduino-wifi` checks the isolation on both chips: `example.com` does not resolve (the phase 0 worry that restricted mode forwards DNS did not hold with QEMU 9.2's libslirp), and TCP to 1.1.1.1 and to the gateway (the host) fails. Its web server answers on the forwarded loopback port (`lcgamboa_*_simulated_wifi_is_isolated_and_serves`). WPA2 is not modelled: the fork's radio is reverse-engineered at register level without the MAC's hardware crypto. Arduino requires WPA2 when a password is given and accepts an open network when it is empty, so the core gives library sketches an empty password through their settings block. Every ESP32 and C3 build of the library's Wi-Fi projects then passes; the S3 has no radio.

## Open questions

- Should sessions run with `-icount` (deterministic, hardware-like timing; see the DHT section), at the cost of single-threaded TCG and work in the fork's library host?
