# ESP-IDF examples in the simulator

Which of ESP-IDF's own examples run in the simulator, as a user opening them as local projects would see. Run on 2026-11-19 with ESP-IDF 6.0.2, fork recipe 12, by `tests/idf-examples/run.sh`: each example is copied out of the ESP-IDF tree, opened with `esp32bench local open`, built for the chip, and run for 15 s (longer where noted). Examples that pull components from Espressif's registry (`get-started/blink`, `wifi/getting_started/station`) were left out.

| Example | ESP32 | ESP32-S3 | ESP32-C3 |
| --- | --- | --- | --- |
| `get-started/hello_world` | Runs, restarts after 10 s | Runs | Runs |
| `peripherals/adc/oneshot_read` | Runs; reads 0 V with no part on the pins | Runs; reads 0 | Runs; reads 0 V |
| `peripherals/gpio/generic_gpio` | Runs, interrupts counted | Runs | Runs |
| `peripherals/timer_group/gptimer` | Runs | Runs | Runs |
| `system/esp_timer` | Timers run; **light sleep resets the chip** (SW_RESET) | Timers run; light sleep wakes 1.5 ms late, and the example's ±1 ms check aborts | Timers run; **light sleep panics** (interrupt watchdog) |
| `system/deep_sleep` | Wakes from the 20 s timer (run for 32 s) | Wakes | Wakes |
| `storage/nvs/nvs_rw_value` | Writes and reads back | Same | Same |
| `system/freertos/real_time_stats` | Runs | Runs | Runs |
| `peripherals/i2c/i2c_tools` | Console prompt | Same | Same |
| `protocols/http_server/simple` | Joins the simulated Wi-Fi, gets 10.0.2.15, serves on port 80 | Same | Same |
| `peripherals/rmt/led_strip_simple_encoder` | Runs (no strip placed) | Runs | Runs |
| `peripherals/ledc/ledc_basic` | Runs | Runs | Runs |
| `system/console/basic` | Console prompt | Same | Same |
| `storage/spiffs` | **`SPIFFS_check()` does not finish** in 70 s; the task watchdog warns every 5 s | Formats, checks, writes and reads | Same |

`http_server/simple` needs the simulated network's name in its configuration: the script adds `CONFIG_EXAMPLE_WIFI_SSID="PICSimLabWifi"`, an empty password, and `CONFIG_EXAMPLE_CONNECT_IPV6=n` (the network has no IPv6).

## Gaps found

- **Light sleep in ESP-IDF builds.** The fork's sleep support (patch 0008) is tested with Arduino builds (`tests/firmware/arduino-sleep`), which keep working. ESP-IDF's default configuration takes another path through `esp_light_sleep_start()`, and it fails on all three chips, each differently (table above). Deep sleep works.
- **SPIFFS on the ESP32.** Formatting and `SPIFFS_check()` are far slower than on the C3 and S3 (formatting took 4.5 s against 0.3 s), and the check does not finish. The S3 and C3 complete in about 2 s.
