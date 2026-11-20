#!/bin/bash
# Smoke-test the Windows .zip as a user would unpack it (Git Bash on a Windows runner):
# the programs, the core over its named pipe, the bundled simulator (the hello firmware
# boots on all three chips; GPIO pins both ways over the pin link), and the UI staying up headless.
#
# Usage: packaging/windows/smoke.sh <zip> <firmware dir with hello-*.bin, gpio-*.bin>
set -euo pipefail

zip=$1
fw=$(cygpath -w "$2")
fail() { echo "smoke: FAIL: $*" >&2; exit 1; }

work=$(mktemp -d)
7z x -o"$work" "$zip" > /dev/null
bin="$work/ESP32 Bench"

"$bin/esp32benchd.exe" --version
"$bin/esp32bench.exe" --version

"$bin/esp32benchd.exe" > "$work/core.log" 2>&1 &
core=$!
trap 'kill $core 2> /dev/null || true' EXIT
for _ in $(seq 50); do "$bin/esp32bench.exe" rpc core.hello > "$work/hello.json" 2> /dev/null && break; sleep 0.2; done
grep -q '"protocol"' "$work/hello.json" || { cat "$work/core.log"; fail "the core does not answer on its pipe"; }
"$bin/esp32bench.exe" rpc toolchain.status > "$work/status.json"
python - "$(cygpath -w "$work/status.json")" <<'PY' || fail "toolchain.status does not see the bundled simulator"
import json, sys
c = {x["id"]: x for x in json.load(open(sys.argv[1]))["components"]}
sim = c["qemu-lcgamboa"]
print("simulator:", sim["state"], sim.get("path"))
assert sim["state"] == "installed", sim
assert "lcgamboa" in sim["path"], sim
PY

for chip in esp32 esp32s3 esp32c3; do
  (sleep 30) | "$bin/esp32bench.exe" sim run --chip $chip --no-network "$fw\\hello-$chip.bin" \
    > "$work/hello-$chip.log" 2> "$work/hello-$chip.err" &
  pid=$!
  ok=
  for _ in $(seq 150); do
    grep -aq "esp32bench-hello ready" "$work/hello-$chip.log" && { ok=1; break; }
    sleep 0.2
  done
  kill $pid 2> /dev/null || true
  taskkill //F //T //PID "$(cat /proc/$pid/winpid 2> /dev/null || echo 0)" > /dev/null 2>&1 || true
  [ -n "$ok" ] || { cat "$work/hello-$chip.err"; tail -20 "$work/hello-$chip.log"; fail "$chip did not boot"; }
  grep -q "engine: lcgamboa" "$work/hello-$chip.err" || fail "$chip did not run on the bundled simulator"
  echo "smoke: $chip boots on the bundled simulator"
done


# Pins, both ways over the host's pin link (loopback TCP on Windows): GPIO 2's toggling
# reaches the core, and GPIO 4 driven from the core reaches the firmware.
(sleep 4; echo ":set 4 1"; sleep 30) | "$bin/esp32bench.exe" sim run --chip esp32c3 --no-network --pins 2,4 \
  "$fw\\gpio-esp32c3.bin" > "$work/gpio.log" 2> "$work/gpio.err" &
pid=$!
ok=
for _ in $(seq 150); do
  grep -aq "esp32bench-gpio in 4=1" "$work/gpio.log" && grep -aq "gpio 2 = 1" "$work/gpio.err" && { ok=1; break; }
  sleep 0.2
done
kill $pid 2> /dev/null || true
wait $pid 2> /dev/null || true
[ -n "$ok" ] || { cat "$work/gpio.err"; tail -20 "$work/gpio.log"; fail "pins did not reach the host and back"; }
echo "smoke: GPIO 2 seen toggling and GPIO 4 driven, over the loopback pin link"

# What the app is for: install the pinned arduino-cli and ESP32 core, build a library
# project, and run the build in the bundled simulator (downloads about 1 GB).
if [ "${ESP32BENCH_SMOKE_BUILD:-}" = 1 ]; then
  "$bin/esp32bench.exe" toolchain install arduino-cli
  "$bin/esp32bench.exe" toolchain install esp32-core
  "$bin/esp32bench.exe" library refresh
  "$bin/esp32bench.exe" build blink --board esp32 --yes > "$work/build.log" 2>&1 \
    || { tail -40 "$work/build.log"; fail "building blink failed"; }
  img=$(find "$(cygpath -u "$LOCALAPPDATA")/esp32bench/cache" -name 'blink.ino.merged.bin' | head -1)
  [ -n "$(cygpath -w "$img")" ] || fail "no blink image under $(cygpath -u "$LOCALAPPDATA")/esp32bench/cache"
  (sleep 30) | "$bin/esp32bench.exe" sim run --chip esp32 --no-network "$(cygpath -w "$img")" > "$work/blink.log" 2> "$work/blink.err" &
  pid=$!
  ok=
  for _ in $(seq 100); do
    grep -aq "entry 0x" "$work/blink.log" && { ok=1; break; }
    sleep 0.2
  done
  sleep 3
  kill $pid 2> /dev/null || true
  wait $pid 2> /dev/null || true
  [ -n "$ok" ] || { cat "$work/blink.err"; tail -20 "$work/blink.log"; fail "the blink build did not start"; }
  grep -aq "Guru Meditation\|abort()" "$work/blink.log" && fail "the blink build crashed"
  echo "smoke: built blink with the app's arduino-cli and ran it in the simulator"
fi

# The user's own ESP-IDF project: with ESP-IDF installed as Espressif's guide does
# (found without IDF_PATH), open a copy of tests/firmware/hello, build it for the S3
# (no shell: ESP-IDF's Python and idf_tools.py export) and boot the build.
if [ "${ESP32BENCH_SMOKE_IDF:-}" = 1 ]; then
  proj="$work/my hello"
  cp -R "$(dirname "$0")/../../tests/firmware/hello" "$proj"
  rm -rf "${proj:?}/build" "${proj:?}/sdkconfig"
  "$bin/esp32bench.exe" local list 2> "$work/idf-list.err" || true
  grep -q "ESP-IDF .* at " "$work/idf-list.err" || { cat "$work/idf-list.err"; fail "ESP-IDF was not found"; }
  id=$("$bin/esp32bench.exe" local open "$(cygpath -w "$proj")" | awk '{ print $1 }')
  [ -n "$id" ] || fail "the ESP-IDF project did not open"
  "$bin/esp32bench.exe" build "$id" --board esp32s3 > "$work/idf-build.log" 2>&1 \
    || { tail -60 "$work/idf-build.log"; fail "building the ESP-IDF project failed"; }
  img=$(find "$(cygpath -u "$LOCALAPPDATA")/esp32bench/cache" -name "$id.ino.merged.bin" | head -1)
  [ -n "$img" ] || fail "no image for $id"
  (sleep 30) | "$bin/esp32bench.exe" sim run --chip esp32s3 --no-network "$(cygpath -w "$img")" > "$work/idf-run.log" 2> "$work/idf-run.err" &
  pid=$!
  ok=
  for _ in $(seq 150); do
    grep -aq "esp32bench-hello ready" "$work/idf-run.log" && { ok=1; break; }
    sleep 0.2
  done
  kill $pid 2> /dev/null || true
  wait $pid 2> /dev/null || true
  [ -n "$ok" ] || { cat "$work/idf-run.err"; tail -20 "$work/idf-run.log"; fail "the ESP-IDF build did not boot"; }
  [ -z "$(ls "$proj" | grep -v '^\(main\|CMakeLists.txt\|README.md\|sdkconfig.defaults\)$')" ] \
    || { ls "$proj"; fail "the build wrote into the project folder"; }
  echo "smoke: built a local ESP-IDF project with ESP-IDF $(sed -n 's/^ESP-IDF \([^ ]*\).*/\1/p' "$work/idf-list.err") and ran it in the simulator"
fi

# Qt's SVG icon engine, without which the bundled icons (ui/icons) and the app's own
# do not draw: Windows has no icon theme of its own.
ls "$bin/iconengines/"*qsvgicon* > /dev/null 2>&1 || fail "the app has no SVG icon engine (iconengines)"
echo "smoke: SVG icon engine bundled"

# The UI starts and stays up headless.
QT_QPA_PLATFORM=offscreen "$bin/esp32bench-gui.exe" --version || true
QT_QPA_PLATFORM=offscreen "$bin/esp32bench-gui.exe" > "$work/gui.log" 2>&1 &
gui=$!
sleep 5
kill -0 $gui 2> /dev/null || { cat "$work/gui.log"; fail "the UI exited instead of running"; }
kill $gui || true
echo "smoke: OK"
