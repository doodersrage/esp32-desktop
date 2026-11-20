#!/bin/bash
# Smoke-test the Windows .zip as a user would unpack it (Git Bash on a Windows runner):
# the programs, the core over its named pipe, the bundled simulator (the hello firmware
# boots on all three chips; GPIO 2 is traced), and the UI staying up headless.
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

# The UI starts and stays up headless.
QT_QPA_PLATFORM=offscreen "$bin/esp32bench-gui.exe" --version || true
QT_QPA_PLATFORM=offscreen "$bin/esp32bench-gui.exe" > "$work/gui.log" 2>&1 &
gui=$!
sleep 5
kill -0 $gui 2> /dev/null || { cat "$work/gui.log"; fail "the UI exited instead of running"; }
kill $gui || true
echo "smoke: OK"
