#!/bin/bash
# Check an installed esp32bench package in a fresh container of its distribution:
# install it with the system's package manager, then the commands, the core over
# its socket, the UI started headless, and the files the package must place.
#
# Usage: smoke.sh <package file>
# With ESP32BENCH_SMOKE_DOWNLOADS=1 it also installs arduino-cli through the app
# where the distribution has no package for it (a 17 MB download).
set -euo pipefail

pkg=$1
. /etc/os-release
echo "== $PRETTY_NAME: installing $(basename "$pkg")"
case "$pkg" in
  *.deb)
    export DEBIAN_FRONTEND=noninteractive
    apt-get update -qq
    apt-get install -y -qq --no-install-recommends "$pkg" > /dev/null
    group=dialout ;;
  *.rpm)
    dnf install -y -q --setopt=install_weak_deps=False "$pkg" > /dev/null
    group=dialout ;;
  *.pkg.tar.zst)
    # A whole-system upgrade first, as on any Arch system (partial upgrades break).
    pacman -Syu --noconfirm > /dev/null
    pacman -U --noconfirm "$pkg" > /dev/null
    group=uucp ;;
  *) echo "unknown package type: $pkg" >&2; exit 2 ;;
esac

fail() { echo "FAIL: $*" >&2; exit 1; }

for f in /usr/bin/esp32bench /usr/bin/esp32benchd /usr/bin/esp32bench-gui \
         /usr/lib/esp32bench/esp32bench-qemu-host \
         /usr/share/applications/esp32bench.desktop \
         /usr/share/icons/hicolor/scalable/apps/esp32bench.svg \
         /usr/lib/udev/rules.d/69-esp32bench.rules; do
  [ -e "$f" ] || fail "missing $f"
done
grep -q "GROUP=\"$group\"" /usr/lib/udev/rules.d/69-esp32bench.rules \
  || fail "the udev rule does not use the $group group"
grep -q 'x-scheme-handler/esp32bench' /usr/share/applications/esp32bench.desktop \
  || fail "the desktop entry does not handle esp32bench:// links"

esp32bench --version
QT_QPA_PLATFORM=offscreen esp32bench-gui --version

# A user's session: the core's socket lives in XDG_RUNTIME_DIR.
export HOME=/tmp/home XDG_RUNTIME_DIR=/tmp/run
mkdir -p "$HOME" "$XDG_RUNTIME_DIR"
chmod 700 "$XDG_RUNTIME_DIR"
esp32benchd > /tmp/core.log 2>&1 &
core=$!
for _ in $(seq 50); do [ -S "$XDG_RUNTIME_DIR/esp32bench/core.sock" ] && break; sleep 0.1; done
esp32bench rpc core.hello | tee /tmp/hello.json
grep -q '"name"' /tmp/hello.json || fail "core.hello did not answer"

# The hints name this distribution's packages and serial group.
esp32bench rpc toolchain.status > /tmp/status.json
case "$ID ${ID_LIKE:-}" in
  *arch*) grep -q 'pacman' /tmp/status.json || fail "no pacman hint" ;;
  *debian*|*ubuntu*)
    grep -q 'installs arduino-cli' /tmp/status.json || fail "arduino-cli is not offered"
    if grep -q 'sudo apt install' /tmp/status.json; then :; else
      grep -q '"qemu-lcgamboa"' /tmp/status.json || fail "no simulator status"
    fi ;;
  *fedora*)
    grep -q 'installs arduino-cli' /tmp/status.json || fail "arduino-cli is not offered" ;;
esac

# The UI starts and stays up, headless, against the running core.
QT_QPA_PLATFORM=offscreen timeout 5 esp32bench-gui > /tmp/gui.log 2>&1 && rc=0 || rc=$?
[ "$rc" = 124 ] || { cat /tmp/gui.log; fail "the UI exited ($rc) instead of running"; }

if [ "${ESP32BENCH_SMOKE_DOWNLOADS:-0}" = 1 ] && grep -q 'installs arduino-cli' /tmp/status.json; then
  esp32bench toolchain install arduino-cli
  "$HOME/.local/share/esp32bench/arduino-cli/"*/arduino-cli version
  esp32bench toolchain status | grep -i 'arduino-cli' | grep -qi 'installed' \
    || fail "arduino-cli not found after installing it"
fi

kill "$core"
echo "== $PRETTY_NAME: OK"
