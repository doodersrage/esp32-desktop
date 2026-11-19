#!/bin/sh
# Install esp32bench's files into a staging tree, the same layout for every package
# (Arch, Debian and Ubuntu, Fedora). The binaries must be built already.
#
# Usage: packaging/install-files.sh <repo> <cargo target dir> <ui build dir> <destdir> <serial group>
#
#   <serial group>  the group that owns serial ports on the target distribution: uucp
#                   on Arch, dialout on Debian, Ubuntu and Fedora. The udev rule gives
#                   ports to it (and to the seated user, through uaccess).
set -eu

repo=$1
target=$2
ui_build=$3
dest=$4
group=$5

case "$group" in
  uucp|dialout) ;;
  *) echo "install-files.sh: unexpected serial group '$group'" >&2; exit 2 ;;
esac

install -Dm755 "$target/release/esp32benchd" "$dest/usr/bin/esp32benchd"
install -Dm755 "$target/release/esp32bench" "$dest/usr/bin/esp32bench"
# The per-session simulator host: a helper the daemon starts, not a command.
install -Dm755 "$target/release/esp32bench-qemu-host" \
  "$dest/usr/lib/esp32bench/esp32bench-qemu-host"
DESTDIR="$dest" cmake --install "$ui_build"

install -Dm644 "$repo/packaging/esp32bench.desktop" "$dest/usr/share/applications/esp32bench.desktop"
install -Dm644 "$repo/packaging/icons/esp32bench.svg" \
  "$dest/usr/share/icons/hicolor/scalable/apps/esp32bench.svg"
install -d "$dest/usr/lib/udev/rules.d"
sed "s/GROUP=\"uucp\"/GROUP=\"$group\"/" "$repo/packaging/udev/69-esp32bench.rules" \
  > "$dest/usr/lib/udev/rules.d/69-esp32bench.rules"
chmod 644 "$dest/usr/lib/udev/rules.d/69-esp32bench.rules"
install -Dm644 "$repo/LICENSE" "$dest/usr/share/licenses/esp32bench/LICENSE"
