#!/usr/bin/env bash
# Build ESP-IDF's own examples through the app's local-project path and run each in the
# simulator for 15 s on the ESP32, S3 and C3, keeping the console of each run.
# Results are in docs/idf-examples.md.
#
# Usage: tests/idf-examples/run.sh <work dir>
#
# Each example is copied into the work dir first, so nothing is written to the ESP-IDF
# tree. The copies are opened as local projects (`esp32bench local open`) and forgotten
# again at the end; build files go to the app's cache, as for any local project.
# ESP32BENCH_CLI=<esp32bench binary> overrides the CLI (default: the debug build).

set -euo pipefail

work="${1:?usage: run.sh <work dir>}"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cli="${ESP32BENCH_CLI:-$here/../../core/target/debug/esp32bench}"
idf="${IDF_PATH:-$HOME/esp/esp-idf}"
mkdir -p "$work/logs"

# No registry components (they would be downloaded), and output on the console.
examples=(get-started/hello_world peripherals/adc/oneshot_read peripherals/gpio/generic_gpio
    peripherals/timer_group/gptimer system/esp_timer system/deep_sleep storage/nvs/nvs_rw_value
    system/freertos/real_time_stats peripherals/i2c/i2c_tools protocols/http_server/simple
    peripherals/rmt/led_strip_simple_encoder peripherals/ledc/ledc_basic system/console/basic
    storage/spiffs)

ids=()
for e in "${examples[@]}"; do
    name="$(basename "$e")"
    rm -rf "${work:?}/$name"
    cp -r "$idf/examples/$e" "$work/$name"
    if [[ "$name" == simple ]]; then
        # Join the simulated network, which is open and has no IPv6.
        printf '%s\n' 'CONFIG_EXAMPLE_WIFI_SSID="PICSimLabWifi"' 'CONFIG_EXAMPLE_WIFI_PASSWORD=""' \
            'CONFIG_EXAMPLE_CONNECT_IPV6=n' >> "$work/$name/sdkconfig.defaults"
    fi
    id="$("$cli" local open "$work/$name" | awk '{print $1}')"
    ids+=("$id")
    for chip in esp32 esp32s3 esp32c3; do
        log="$work/logs/$name-$chip"
        if ! "$cli" build "$id" --board "$chip" > "$log.build" 2>&1; then
            echo "$name $chip: build failed ($log.build)"
            continue
        fi
        image="$HOME/.cache/esp32bench/builds/$id/$chip/out/$id.ino.merged.bin"
        sleep 15 | timeout --signal=INT 15 "$cli" sim run --chip "$chip" "$image" > "$log.run" 2>&1 || true
        echo "$name $chip: $log.run"
    done
done
for id in "${ids[@]}"; do "$cli" local remove "$id"; done
