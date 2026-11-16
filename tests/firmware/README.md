# Boot-test firmware

Small firmware projects that the core's QEMU boot tests run. The ESP-IDF version is pinned in `IDF_VERSION`.

| Project | What it does | Marker on UART0 |
| --- | --- | --- |
| `hello` | Prints a ready line, a tick every second, and echoes lines received on UART0 | `esp32d-hello ready`, `esp32d-hello echo <line>` |
| `wifi-sta` | Joins the open network `PICSimLabWifi`. Only lcgamboa's QEMU fork emulates it, on ESP32 and C3 | `esp32d-wifi got ip <ip>` |

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
```

The script refuses to run against any other IDF version. `cargo test --manifest-path core/Cargo.toml` picks up the images automatically.

Projects use DIO flash mode and 4 MiB flash in `sdkconfig.defaults`, because QEMU boots only DIO or DOUT images.
