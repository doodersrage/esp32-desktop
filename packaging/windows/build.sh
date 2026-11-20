#!/bin/bash
# Assemble ESP32 Bench for Windows, from finished builds (run in Git Bash on a
# Windows runner):
#   - the core's release binaries (cargo build --release),
#   - the UI (cmake --build, MSVC), with Qt deployed beside it by windeployqt, and
#   - lcgamboa's QEMU fork, patched, built statically with MSYS2 and linked as DLLs
#     (tests/platforms/link-library.sh).
# The simulator ships with the app (docs/platforms.md): its DLLs go to lcgamboa\ laid
# out as the setup step installs it on Linux, so lcgamboa::Build finds it
# (bundled_root). Makes a folder, a .zip of it and, with Inno Setup, an installer that
# registers esp32bench:// links.
#
# Usage: packaging/windows/build.sh <repo> <cargo target dir> <ui build dir> <sim dir> <out dir>
#   <sim dir> holds build/libqemu-*.dll, pc-bios/ and COMMIT.
set -euo pipefail

repo=$1
target=$2
ui_build=$3
sim_src=$4
out=$5

version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$repo/core/Cargo.toml" | head -1)
app="$out/ESP32 Bench"
rm -rf "$app"
mkdir -p "$app"

cp "$ui_build/esp32bench-gui.exe" "$app/"
for b in esp32benchd esp32bench esp32bench-qemu-host; do
  cp "$target/release/$b.exe" "$app/"
done
windeployqt --release --no-compiler-runtime "$app/esp32bench-gui.exe"

sim="$app/lcgamboa"
mkdir -p "$sim/src/build"
cp "$sim_src"/build/libqemu-xtensa.dll "$sim_src"/build/libqemu-riscv32.dll "$sim/src/build/"
cp -R "$sim_src/pc-bios" "$sim/src/pc-bios"
commit=$(cat "$sim_src/COMMIT")
recipe=$(sed -n 's/^pub const RECIPE: u32 = \([0-9]*\);/\1/p' "$repo/core/crates/sim/src/lcgamboa.rs")
printf '%s\nrecipe %s\n' "$commit" "$recipe" > "$sim/installed"

cp "$repo/LICENSE" "$app/LICENSE.txt"
cat > "$app/SOURCE.txt" <<TXT
ESP32 Bench $version is free software (GPL-2.0-or-later): https://github.com/doodersrage/esp32-desktop
The simulator in lcgamboa\\ is lcgamboa's QEMU (GPL-2.0) at
https://github.com/lcgamboa/qemu commit $commit, with the patches in
core/crates/sim/patches/ of the source above (fork recipe $recipe), built
statically with MSYS2 (UCRT64).
TXT

(cd "$out" && 7z a -tzip "esp32bench-$version-windows-x86_64.zip" "ESP32 Bench" > /dev/null)
echo "built $out/esp32bench-$version-windows-x86_64.zip"

iscc=""
for c in "/c/Program Files (x86)/Inno Setup 6/ISCC.exe" "/c/Program Files/Inno Setup 6/ISCC.exe"; do
  [ -f "$c" ] && iscc=$c
done
if [ -n "$iscc" ]; then
  # Absolute paths: Inno Setup resolves relative ones against the script's folder.
  "$iscc" //Qp "//DVersion=$version" "//DSource=$(cygpath -w "$(cd "$app" && pwd)")" \
    "//O$(cygpath -w "$(cd "$out" && pwd)")" \
    "$(cygpath -w "$repo/packaging/windows/esp32bench.iss")"
  echo "built $out/esp32bench-$version-windows-x86_64-setup.exe"
else
  echo "no Inno Setup: no installer" >&2
fi
