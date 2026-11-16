#!/usr/bin/env bash
# Build boot-test firmware for every simulated chip and write merged 4 MiB flash
# images to tests/firmware/out/<project>-<chip>.bin.
#
# Usage: tests/firmware/build.sh [project]   (default: hello)
#
# ESP-IDF projects need IDF_PATH pointing at a checkout of the version in IDF_VERSION.
# Arduino sketches (a directory with <project>.ino) need arduino-cli with the ESP32
# core version in ARDUINO_ESP32_VERSION installed.

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
project="${1:-hello}"
chips=(esp32 esp32s3 esp32c3)
src="$here/$project"
out="$here/out"
mkdir -p "$out"

if [[ -f "$src/$project.ino" ]]; then
    want="$(<"$here/ARDUINO_ESP32_VERSION")"
    have="$(arduino-cli core list | awk '$1 == "esp32:esp32" { print $2 }')"
    if [[ "$have" != "$want" ]]; then
        echo "error: esp32:esp32 core is '${have:-not installed}', tests are pinned to $want" >&2
        exit 1
    fi
    for chip in "${chips[@]}"; do
        build="$here/build/$project-$chip"
        echo "== $project for $chip (Arduino core $want)"
        # DIO and 4 MiB because QEMU boots only DIO or DOUT images. CDC stays off
        # on S3 and C3 so Serial is UART0.
        fqbn="esp32:esp32:$chip:FlashMode=dio,FlashSize=4M"
        [[ "$chip" != esp32 ]] && fqbn="$fqbn,CDCOnBoot=default"
        arduino-cli compile --fqbn "$fqbn" --build-path "$build" "$src"
        cp "$build/$project.ino.merged.bin" "$out/$project-$chip.bin"
    done
    echo "images in $out"
    exit 0
fi

want="$(<"$here/IDF_VERSION")"
: "${IDF_PATH:?set IDF_PATH to an ESP-IDF $want checkout}"
have="$(git -C "$IDF_PATH" describe --tags --exact-match 2>/dev/null || true)"
if [[ "$have" != "$want" ]]; then
    echo "error: IDF_PATH is at '${have:-unknown}', tests are pinned to $want" >&2
    exit 1
fi

# shellcheck source=/dev/null
. "$IDF_PATH/export.sh" >/dev/null

for chip in "${chips[@]}"; do
    build="$here/build/$project-$chip"
    echo "== $project for $chip"
    idf.py -C "$src" -B "$build" \
        -D SDKCONFIG="$build/sdkconfig" \
        -D SDKCONFIG_DEFAULTS="$src/sdkconfig.defaults" \
        set-target "$chip" build
    idf.py -C "$src" -B "$build" -D SDKCONFIG="$build/sdkconfig" \
        merge-bin --pad-to-size 4MB -o "$out/$project-$chip.bin"
done

echo "images in $out"
