# ESP-IDF examples in the simulator

Which of ESP-IDF's own examples run in the simulator, as a user opening them as local projects would see. Run on 2026-11-19 with ESP-IDF 6.0.2, fork recipe 12, then the two rows marked "recipe 13" again after patch 0013, by `tests/idf-examples/run.sh`: each example is copied out of the ESP-IDF tree, opened with `esp32bench local open`, built for the chip, and run for 15 s (longer where noted). Examples that pull components from Espressif's registry (`get-started/blink`, `wifi/getting_started/station`) were left out.

| Example | ESP32 | ESP32-S3 | ESP32-C3 |
| --- | --- | --- | --- |
| `get-started/hello_world` | Runs, restarts after 10 s | Runs | Runs |
| `peripherals/adc/oneshot_read` | Runs; reads 0 V with no part on the pins | Runs; reads 0 | Runs; reads 0 V |
| `peripherals/gpio/generic_gpio` | Runs, interrupts counted | Runs | Runs |
| `peripherals/timer_group/gptimer` | Runs | Runs | Runs |
| `system/esp_timer` (recipe 13) | Timers run; light sleep wakes, 1.6 ms late, so the example's +1 ms check aborts; with chip-like timing (`--icount`) it passes | Wakes 1.3 ms late (3 ms with chip-like timing): the check aborts | Wakes 1.1 ms late; passes with chip-like timing |
| `system/deep_sleep` | Wakes from the 20 s timer (run for 32 s) | Wakes | Wakes |
| `storage/nvs/nvs_rw_value` | Writes and reads back | Same | Same |
| `system/freertos/real_time_stats` | Runs | Runs | Runs |
| `peripherals/i2c/i2c_tools` | Console prompt | Same | Same |
| `protocols/http_server/simple` | Joins the simulated Wi-Fi, gets 10.0.2.15, serves on port 80 | Same | Same |
| `peripherals/rmt/led_strip_simple_encoder` | Runs (no strip placed) | Runs | Runs |
| `peripherals/ledc/ledc_basic` | Runs | Runs | Runs |
| `system/console/basic` | Console prompt | Same | Same |
| `storage/spiffs` (recipe 13) | Formats, checks (about 7 s, with one task watchdog warning), writes and reads | Formats, checks (2 s), writes and reads | Same (1.5 s) |

`http_server/simple` needs the simulated network's name in its configuration: the script adds `CONFIG_EXAMPLE_WIFI_SSID="PICSimLabWifi"`, an empty password, and `CONFIG_EXAMPLE_CONNECT_IPV6=n` (the network has no IPv6).

## Gaps found, and fixed

On recipe 12 these two failed; patch 0013 (`docs/qemu-forks.md`) fixed both.

- **Light sleep in ESP-IDF builds.** The interrupt watchdog fired during the second light sleep on the ESP32 (a TG1 reset) and the C3 (a panic): the timer groups kept counting while the chip slept. They now stand still, as the chip's gated clock makes them. Arduino builds took another path and were not affected. What is left is timing: ESP-IDF shortens the first sleep by the wake-up time it expects of real hardware and then adapts, and without chip-like timing its entry and exit code takes longer in simulated time than on the chip, so one sleep can be a millisecond or two off.
- **SPIFFS on the ESP32.** `SPIFFS_check()` did not finish in 70 s: every flash read turned the cache off and on, and the model remapped memory each time. It now takes about 7 s.
