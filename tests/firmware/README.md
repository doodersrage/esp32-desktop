# Boot-test firmware

Small firmware projects that the core's QEMU boot tests run. The ESP-IDF version is pinned in `IDF_VERSION` and the ESP32 Arduino core version in `ARDUINO_ESP32_VERSION`.

| Project | What it does | Marker on UART0 |
| --- | --- | --- |
| `hello` | Prints a ready line, a tick every second, and echoes lines received on UART0 | `esp32bench-hello ready`, `esp32bench-hello echo <line>` |
| `hello-arduino` | Arduino sketch: ready line, ticks and echo, built with the pinned ESP32 Arduino core | `esp32bench-arduino ready` |
| `gpio` | Toggles GPIO 2 every 500 ms and reports changes on GPIO 4 | `esp32bench-gpio ready`, `esp32bench-gpio in 4=<level>` |
| `i2c-scan` | Scans I2C bus 0 and reads registers 0x10–0x11 from 0x3C | `esp32bench-i2c scan done count=<n>`, `esp32bench-i2c read ...` |
| `arduino-periph` | Arduino sketch: analogRead, tone, analogWrite, a 192-symbol rmtWrite and rgbLedWrite (ADC, LEDC and RMT, which library sketches use) | `esp32bench-periph done`, `esp32bench-periph rmt=1` |
| `arduino-sleep` | Arduino sketch: 200 ms of light sleep, then deep sleep on a timer twice, counting boots in RTC memory | `esp32bench-sleep boot=<n> cause=<c> reset=<r>`, `esp32bench-sleep done` |
| `wifi-sta` | Joins the open network `PICSimLabWifi`. Only lcgamboa's QEMU fork emulates it, on ESP32 and C3 | `esp32bench-wifi got ip <ip>` |

## Building

Check out the pinned IDF and install its tools for the simulated chips:

```sh
git clone --branch "$(cat tests/firmware/IDF_VERSION)" --depth 1 --recurse-submodules --shallow-submodules \
    https://github.com/espressif/esp-idf.git ~/esp/esp-idf
~/esp/esp-idf/install.sh esp32,esp32s3,esp32c3
```

Then build merged 4 MiB images into `tests/firmware/out/`:

```sh
IDF_PATH=~/esp/esp-idf tests/firmware/build.sh            # hello
IDF_PATH=~/esp/esp-idf tests/firmware/build.sh wifi-sta
IDF_PATH=~/esp/esp-idf tests/firmware/build.sh gpio
IDF_PATH=~/esp/esp-idf tests/firmware/build.sh i2c-scan
tests/firmware/build.sh hello-arduino                  # needs arduino-cli and the pinned core
```

For the Arduino core:

```sh
arduino-cli core install "esp32:esp32@$(cat tests/firmware/ARDUINO_ESP32_VERSION)" \
    --additional-urls https://espressif.github.io/arduino-esp32/package_esp32_index.json
```

The script refuses to run against any other IDF version. `cargo test --manifest-path core/Cargo.toml` picks up the images automatically.

Projects use DIO flash mode and 4 MiB flash in `sdkconfig.defaults`, because QEMU boots only DIO or DOUT images.
