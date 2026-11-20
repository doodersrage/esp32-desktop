#!/bin/bash
# Smoke-test the macOS .dmg as a user would get it: mount it, copy the app out, and
# check the programs, the core over its socket, the bundled simulator (the hello
# firmware boots; GPIO 2 toggles and is traced), and the UI staying up headless.
#
# Usage: packaging/macos/smoke.sh <dmg> <firmware dir with hello-esp32.bin, gpio-*.bin>
set -euo pipefail

dmg=$1
fw=$2
fail() { echo "smoke: FAIL: $*" >&2; exit 1; }

work=$(mktemp -d)
mnt="$work/mnt"
hdiutil attach -nobrowse -readonly -mountpoint "$mnt" "$dmg" > /dev/null
cp -R "$mnt/ESP32 Bench.app" "$work/"
hdiutil detach "$mnt" > /dev/null
bin="$work/ESP32 Bench.app/Contents/MacOS"

"$bin/esp32benchd" --version
"$bin/esp32bench" --version
codesign --verify --deep --strict "$work/ESP32 Bench.app" || fail "the app's signature is not valid"

# The core, on its default socket ($TMPDIR on macOS).
"$bin/esp32benchd" > "$work/core.log" 2>&1 &
core=$!
trap 'kill $core 2> /dev/null || true' EXIT
for _ in $(seq 50); do "$bin/esp32bench" rpc core.hello > "$work/hello.json" 2> /dev/null && break; sleep 0.2; done
grep -q '"protocol"' "$work/hello.json" || { cat "$work/core.log"; fail "the core does not answer"; }
"$bin/esp32bench" rpc toolchain.status > "$work/status.json"
python3 - "$work/status.json" <<'PY' || fail "toolchain.status does not see the bundled simulator"
import json, sys
c = {x["id"]: x for x in json.load(open(sys.argv[1]))["components"]}
sim = c["qemu-lcgamboa"]
print("simulator:", sim["state"], sim.get("path"))
assert sim["state"] == "installed", sim
assert "Resources/lcgamboa" in sim["path"], sim
PY

# The bundled simulator, through the host and the library.
for chip in esp32 esp32s3 esp32c3; do
  (sleep 20) | "$bin/esp32bench" sim run --chip $chip --no-network "$fw/hello-$chip.bin" \
    > "$work/hello-$chip.log" 2> "$work/hello-$chip.err" &
  pid=$!
  ok=
  for _ in $(seq 100); do
    grep -aq "esp32bench-hello ready" "$work/hello-$chip.log" && { ok=1; break; }
    sleep 0.2
  done
  kill -INT $pid 2> /dev/null || true
  wait $pid 2> /dev/null || true
  [ -n "$ok" ] || { cat "$work/hello-$chip.err"; tail -20 "$work/hello-$chip.log"; fail "$chip did not boot"; }
  grep -q "engine: lcgamboa" "$work/hello-$chip.err" || fail "$chip did not run on the bundled simulator"
  echo "smoke: $chip boots on the bundled simulator"
done
(sleep 30) | "$bin/esp32bench" sim run --chip esp32c3 --no-network --pins 2,4 \
  --trace "$work/gpio.vcd" "$fw/gpio-esp32c3.bin" > /dev/null 2> "$work/gpio.err" &
pid=$!
sleep 8
kill -INT $pid 2> /dev/null || true
wait $pid 2> /dev/null || true
grep -q '\$var wire 1 ! GPIO2' "$work/gpio.vcd" || { cat "$work/gpio.err"; fail "no GPIO trace"; }
echo "smoke: GPIO 2 traced ($(grep -c '^#' "$work/gpio.vcd") times)"

# The UI starts and stays up headless.
QT_QPA_PLATFORM=offscreen "$bin/esp32bench-gui" --version
QT_QPA_PLATFORM=offscreen "$bin/esp32bench-gui" > "$work/gui.log" 2>&1 &
gui=$!
sleep 5
kill -0 $gui 2> /dev/null || { cat "$work/gui.log"; fail "the UI exited instead of running"; }
kill $gui
echo "smoke: OK"
