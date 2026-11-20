#!/bin/bash
# Assemble ESP32 Bench.app and a .dmg on macOS, from finished builds:
#   - the core's release binaries (cargo build --release),
#   - the UI (cmake --build), and
#   - lcgamboa's QEMU fork, patched and linked as libraries
#     (tests/platforms/link-library.sh), at <fork>/build/libqemu-*.dylib.
# The simulator ships inside the app (docs/platforms.md): its library goes to
# Contents/Resources/lcgamboa laid out as the setup step installs it on Linux, with the
# Homebrew libraries it links copied beside it (dylibbundler), so lcgamboa::Build
# finds it (bundled_root) and nothing else needs installing.
#
# Usage: packaging/macos/build.sh <repo> <cargo target dir> <ui build dir> <fork tree> <out dir>
set -euo pipefail

repo=$1
target=$2
ui_build=$3
fork=$4
out=$5

version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$repo/core/Cargo.toml" | head -1)
arch=$(uname -m)
app="$out/ESP32 Bench.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources" "$out"

sed "s/@VERSION@/$version/g" "$repo/packaging/macos/Info.plist.in" > "$app/Contents/Info.plist"

# Programs: the app, the core it starts, the CLI and the simulator host, together.
cp "$ui_build/esp32bench-gui" "$app/Contents/MacOS/"
for b in esp32benchd esp32bench esp32bench-qemu-host; do
  cp "$target/release/$b" "$app/Contents/MacOS/"
done

# The icon, from the SVG.
iconset=$(mktemp -d)/esp32bench.iconset
mkdir -p "$iconset"
for s in 16 32 128 256 512; do
  rsvg-convert -w $s -h $s "$repo/packaging/icons/esp32bench.svg" -o "$iconset/icon_${s}x${s}.png"
  rsvg-convert -w $((s * 2)) -h $((s * 2)) "$repo/packaging/icons/esp32bench.svg" -o "$iconset/icon_${s}x${s}@2x.png"
done
iconutil -c icns "$iconset" -o "$app/Contents/Resources/esp32bench.icns"

# The simulator, as lcgamboa::Build expects it.
sim="$app/Contents/Resources/lcgamboa"
mkdir -p "$sim/src/build"
cp "$fork"/build/libqemu-xtensa.dylib "$fork"/build/libqemu-riscv32.dylib "$sim/src/build/"
cp -R "$fork/pc-bios" "$sim/src/pc-bios"
# The fork tree, or a copy of what the app needs from it (build/, pc-bios/, COMMIT).
if [ -f "$fork/COMMIT" ]; then commit=$(cat "$fork/COMMIT"); else commit=$(git -C "$fork" rev-parse HEAD); fi
recipe=$(sed -n 's/^pub const RECIPE: u32 = \([0-9]*\);/\1/p' "$repo/core/crates/sim/src/lcgamboa.rs")
printf '%s\nrecipe %s\n' "$commit" "$recipe" > "$sim/installed"
# Their Homebrew libraries (glib, pixman, libgcrypt, libslirp...), beside them.
dylibbundler -of -b -cd -d "$sim/src/build/libs" -p @loader_path/libs/ \
  -x "$sim/src/build/libqemu-xtensa.dylib" -x "$sim/src/build/libqemu-riscv32.dylib"
# dylibbundler can add the same LC_RPATH more than once, and dyld refuses to load a
# library with a duplicate ("duplicate LC_RPATH"): keep one of each.
find "$sim/src/build" -name '*.dylib' | while read -r lib; do
  otool -l "$lib" | awk '/cmd LC_RPATH/ { getline; getline; print $2 }' | sort | uniq -d |
    while read -r rp; do
      n=$(otool -l "$lib" | awk -v rp="$rp" '/cmd LC_RPATH/ { getline; getline; if ($2 == rp) c++ } END { print c }')
      while [ "$n" -gt 1 ]; do
        install_name_tool -delete_rpath "$rp" "$lib"
        n=$((n - 1))
      done
    done
done

# Qt's frameworks and plugins for the UI.
macdeployqt "$app" -always-overwrite

# The licence and where the simulator's source is (GPL).
cp "$repo/LICENSE" "$app/Contents/Resources/LICENSE"
cat > "$app/Contents/Resources/SOURCE.txt" <<TXT
ESP32 Bench $version is free software (GPL-2.0-or-later): https://github.com/doodersrage/esp32-desktop
The simulator in Resources/lcgamboa is lcgamboa's QEMU (GPL-2.0) at
https://github.com/lcgamboa/qemu commit $commit, with the patches in
core/crates/sim/patches/ of the source above (fork recipe $recipe).
TXT

# Apple silicon runs only signed code: sign it all ad hoc (no identity; Gatekeeper
# still asks the user to allow an app from the internet).
codesign --force --deep --sign - "$app"
codesign --verify --deep --strict "$app"

dmg="$out/esp32bench-$version-macos-$arch.dmg"
rm -f "$dmg"
staging=$(mktemp -d)
cp -R "$app" "$staging/"
ln -s /Applications "$staging/Applications"
hdiutil create -volname "ESP32 Bench $version" -srcfolder "$staging" -ov -format UDZO "$dmg"
echo "built $dmg"
