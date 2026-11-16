#!/usr/bin/env bash
# Build the boot-test firmware for every simulated chip with the pinned ESP-IDF,
# and write merged 4 MiB flash images to tests/firmware/out/<project>-<chip>.bin.
#
# Usage: tests/firmware/build.sh [project]   (default: hello)
# Needs IDF_PATH pointing at a checkout of the version in IDF_VERSION.

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
project="${1:-hello}"
want="$(<"$here/IDF_VERSION")"
chips=(esp32 esp32s3 esp32c3)

: "${IDF_PATH:?set IDF_PATH to an ESP-IDF $want checkout}"
have="$(git -C "$IDF_PATH" describe --tags --exact-match 2>/dev/null || true)"
if [[ "$have" != "$want" ]]; then
    echo "error: IDF_PATH is at '${have:-unknown}', tests are pinned to $want" >&2
    exit 1
fi

# shellcheck source=/dev/null
. "$IDF_PATH/export.sh" >/dev/null

src="$here/$project"
out="$here/out"
mkdir -p "$out"

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
