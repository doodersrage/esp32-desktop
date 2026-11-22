#!/usr/bin/env bash
# Take the README's and esp32bench.dev's pictures of the app (docs/images/), from
# the real app against the real core, off-screen.
#
# Needs: the core and the UI built with -DESP32BENCH_SCREENSHOTS=ON, lcgamboa's fork
# built (`esp32bench toolchain install qemu-lcgamboa`), and the pictured projects
# built for the ESP32. This script builds them with `esp32bench build --yes`: you
# agree to building those library projects by running it.
#
# The app gets its own settings, data and runtime folders, so your layout,
# circuits, stars and breakpoints are neither read nor changed; it shares only the
# simulator and Arduino tools (linked) and the build cache.
set -euo pipefail

repo=$(cd "$(dirname "$0")/../.." && pwd)
out=${1:-$repo/docs/images}
daemon=${ESP32BENCH_DAEMON:-$repo/core/target/debug/esp32benchd}
cli=$repo/core/target/debug/esp32bench
shots=$repo/build/ui/esp32bench-screenshots
data=${XDG_DATA_HOME:-$HOME/.local/share}/esp32bench

for f in "$daemon" "$cli" "$shots"; do
    [ -x "$f" ] || { echo "missing $f: build the core, and the UI with -DESP32BENCH_SCREENSHOTS=ON" >&2; exit 1; }
done

work=$(mktemp -d)
cleanup() {
    # The core the app started, found by its runtime folder.
    for p in $(pgrep -x esp32benchd || true); do
        if tr '\0' '\n' < "/proc/$p/environ" 2>/dev/null | grep -qx "XDG_RUNTIME_DIR=$work/run"; then
            kill "$p" || true
        fi
    done
    rm -rf "${work:?}"
}
trap cleanup EXIT

mkdir -p -m 700 "$work/run" "$work/config" "$work/data/esp32bench"
for d in qemu arduino; do
    [ -e "$data/$d" ] && ln -s "$data/$d" "$work/data/esp32bench/$d"
done

export XDG_RUNTIME_DIR=$work/run XDG_CONFIG_HOME=$work/config XDG_DATA_HOME=$work/data
# This core builds the projects and serves the app, so the app finds them built and
# runs them without asking (the build here is the consent).
"$daemon" > "$work/core.log" 2>&1 &
for _ in $(seq 1 100); do
    [ -S "$work/run/esp32bench/core.sock" ] && break
    sleep 0.1
done
for project in oled blink; do
    "$cli" build "$project" --board esp32 --yes > "$work/build.log" 2>&1 \
        || { cat "$work/build.log" >&2; exit 1; }
done

mkdir -p "$out"
style=()
[ -e /usr/lib/qt6/plugins/styles/breeze6.so ] && style=(-style Breeze)
ESP32BENCH_DAEMON=$daemon QT_QPA_PLATFORM=offscreen LANG=en_US.UTF-8 \
    "$shots" "${style[@]}" "$out"
