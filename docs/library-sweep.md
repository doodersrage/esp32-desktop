# Library sweep

The v1 check in `PLAN.md`: every esp32bench.dev library project builds for every board it lists, and every ESP32, ESP32-S3 and ESP32-C3 build boots in the simulator. `tests/library/sweep.py` does both through the CLI and checks each boot against the site's lab expectation for the project (esp32-bench `src/data/lab-expect.ts`: what the board prints with or without its parts). Run it after changes to the build or the simulator:

```sh
(cd <esp32-bench> && node --experimental-strip-types -e \
  "import('./src/data/lab-expect.ts').then(m => console.log(JSON.stringify(m.LAB_EXPECT)))") > expect.json
python3 tests/library/sweep.py --cli core/target/debug/esp32bench --expect expect.json --out sweep/
```

## First sweep (2026-11-18)

Library: 55 projects from esp32bench.dev, 255 project × board builds. Arduino core 3.3.12, lcgamboa's fork at 885a6e7061 (recipe 4).

**Builds: 250 of 251 sketch builds succeed** (4 more are prebuilt firmware). The one failure: `ble-scan` on the XIAO ESP32-S3 needs the `huge_app` partition scheme, which the build crate says that board lacks.

**Boots: 129 of 227 simulated builds print their expected output.** The rest, by cause:

| Cause | Builds | What happens | Where |
| --- | --- | --- | --- |
| The C3 SuperMini's console is its own USB (USB CDC on boot; the board has no USB-UART chip). Neither QEMU models the USB-Serial-JTAG console: lcgamboa's and Espressif's are both empty stubs. | 41 (37 silent, 4 also crash below) | Boots, prints nothing visible | `esp32c3-supermini` |
| The S3's SAR ADC is not modelled. Any sketch that links the ADC driver runs `adc_hw_calibration()` before `setup()`, which polls `SENS.sar_meas1_ctrl2.meas1_done_sar` (0x6000880C bit 16) forever. Found with GDB. | about 14 | Hangs before `setup()`, silent | `esp32s3` (both QEMUs) |
| RMT transmit never completes, so WS2812 writes block: NeoPixel strips and the C3 DevKitM's on-board RGB LED. | 6 seen (more hidden behind other causes) | Hangs at the first LED write | ESP32, C3 |
| LEDC registers do not read back (the C3 model returns only timer and channel config; the S3 has none), so `tone()` and the LEDC driver's checks abort. | 7 | `abort()` in `ledc_get_freq` / `ledc_ll_get_duty` | C3, S3 |
| Wi-Fi: sketches that wait for a network wait forever, by design (no network without consent). | 23 | Starts, waits | all |
| Deep sleep is not modelled: entering it stalls the chip until the watchdog resets it, and RTC memory is lost. | 4 | Resets every ~1.5 s, "Boot 1" each time | ESP32 (C3: interrupt watchdog panic) |
| Bluetooth: no radio. The BT controller's start-up asserts. | 5 | `assert failed: btdm_low_power_mode_init` | all |
| `stepper-28byj` on ESP32 and S3 crashed once (`LoadProhibited`) and not in three reruns. | 2 | Intermittent | ESP32, S3 |

Wi-Fi, Bluetooth and deep sleep are outside what the simulator claims. The SuperMini console, the S3 ADC, RMT and LEDC are gaps in the QEMU models that sketches in the library hit, and each can be patched in the fork the way the clock fix was.

## After fork patches 0004 and 0005 (2026-11-18)

The same builds booted again on recipe 5 (S3 ADC and LEDC registers, C3 LEDC readback; `docs/qemu-forks.md`). **141 of 227 pass, up from 129; crashes 12, down from 19.** Fourteen builds changed, none for the worse:

- Seven S3 sketches that hung in ADC calibration before `setup()` now run: `auto-fan`, `battery-sensor`, `ir-sweep`, `joystick-turret`, `oled-pong`, `sd-logger` and `talking-plant`.
- `tone()` no longer aborts: `simon` and `morse-decoder` pass on the C3, `simon` and `keypad-lock` on the S3; on the SuperMini, `simon` and `morse-decoder` now run silently (USB console).
- `stepper-28byj` on the ESP32 passed this time; it stays listed as intermittent, with its S3 build.

Left: the SuperMini console (39 builds), RMT for WS2812 LEDs, Wi-Fi waiting by design, deep sleep (4) and Bluetooth (5), which the simulator does not claim, and the intermittent stepper crash on the S3.
