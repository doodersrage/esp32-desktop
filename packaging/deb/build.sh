#!/bin/bash
# Build esp32bench's .deb inside a Debian or Ubuntu container (packaging/ci/build.sh
# runs it in each supported release), against that release's Qt and libraries.
#
# Usage: build.sh <repo, read-only is fine> <output dir>
# The package's version is <version>-1~<distro><release> (the ~ sorts it before a
# plain -1); its file is esp32bench_<version>-1.<distro><release>_amd64.deb, because
# GitHub turns ~ into . in release asset names.
set -euo pipefail

repo=$1
out=$2
export DEBIAN_FRONTEND=noninteractive

apt-get update -qq
apt-get install -y -qq --no-install-recommends \
  build-essential cmake ninja-build pkg-config curl ca-certificates git \
  qt6-base-dev qt6-base-dev-tools qt6-tools-dev qt6-l10n-tools libudev-dev dpkg-dev file > /dev/null

# Rust from rustup: the distributions' own (Debian 13: 1.85) are older than the
# workspace needs (let chains, 1.88).
if ! command -v cargo > /dev/null; then
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
    | sh -s -- -y --profile minimal --default-toolchain stable > /dev/null
fi
export PATH="$HOME/.cargo/bin:$PATH"

work=$(mktemp -d)
# The repository's files only (tracked, or new and not ignored): no build output.
mkdir "$work/src"
git -c safe.directory='*' -C "$repo" ls-files -z --cached --others --exclude-standard \
  | (cd "$repo" && tar --null --files-from=- -cf -) | tar -xf - -C "$work/src"
cd "$work/src"
export CARGO_TARGET_DIR="$work/target"
cargo build --manifest-path core/Cargo.toml --locked --release --bins
cargo test --manifest-path core/Cargo.toml --locked --release --workspace --lib
cmake -S ui -B "$work/build-ui" -G Ninja -DCMAKE_BUILD_TYPE=Release \
  -DCMAKE_INSTALL_PREFIX=/usr -DBUILD_TESTING=OFF > /dev/null
cmake --build "$work/build-ui"

version=$(sed -n 's/^version = "\(.*\)"/\1/p' core/Cargo.toml | head -1)
. /etc/os-release
debver="${version}-1~${ID}${VERSION_ID}"
pkg="$work/pkg"
sh packaging/install-files.sh "$PWD" "$CARGO_TARGET_DIR" "$work/build-ui" "$pkg" dialout
# Debian keeps the licence as the package's copyright file.
install -Dm644 "$pkg/usr/share/licenses/esp32bench/LICENSE" "$pkg/usr/share/doc/esp32bench/copyright"
rm -r "$pkg/usr/share/licenses"

# Library dependencies, from the binaries (dpkg-shlibdeps wants a debian/control).
mkdir -p "$work/shlibs/debian"
printf 'Source: esp32bench\n\nPackage: esp32bench\nArchitecture: any\n' > "$work/shlibs/debian/control"
libdeps=$(cd "$work/shlibs" && dpkg-shlibdeps -O \
  "$pkg/usr/bin/esp32benchd" "$pkg/usr/bin/esp32bench" "$pkg/usr/bin/esp32bench-gui" \
  "$pkg/usr/lib/esp32bench/esp32bench-qemu-host" 2> /dev/null | sed 's/^shlibs:Depends=//')

# Plugins Qt loads at run time are not library dependencies: the SVG icon plugin, and
# for the X11 platform plugin (also what Qt falls back to on Wayland without
# qt6-wayland) libxcb-cursor0, which Qt needs since 6.5 and its packages do not pull in.
svg=qt6-svg-plugins
apt-cache show "$svg" > /dev/null 2>&1 || svg=libqt6svg6

mkdir -p "$pkg/DEBIAN"
size=$(du -sk "$pkg/usr" | cut -f1)
cat > "$pkg/DEBIAN/control" <<CONTROL
Package: esp32bench
Version: $debver
Architecture: amd64
Maintainer: esp32bench <https://github.com/doodersrage/esp32-desktop/issues>
Installed-Size: $size
Depends: $libdeps, $svg, libxcb-cursor0, hicolor-icon-theme, udev, curl, ca-certificates, tar, xz-utils
Recommends: qt6-wayland, git, gcc, make, pkgconf, ninja-build, python3, python3-venv, perl, libglib2.0-dev, libpixman-1-dev, libgcrypt20-dev, libslirp-dev
Section: electronics
Priority: optional
Homepage: https://esp32bench.dev/desktop/
Description: ESP32 workbench and simulator
 Browse and build the esp32bench.dev project library and your own ESP-IDF
 projects, flash and monitor ESP32 boards, and run builds in a simulator with
 virtual parts. The recommended packages build the simulator (lcgamboa's QEMU)
 on this computer; arduino-cli is installed by the app.
CONTROL
cat > "$pkg/DEBIAN/postinst" <<'POSTINST'
#!/bin/sh
set -e
if [ "$1" = configure ] && command -v udevadm > /dev/null; then
  udevadm control --reload-rules > /dev/null 2>&1 || true
  udevadm trigger --subsystem-match=tty > /dev/null 2>&1 || true
fi
POSTINST
chmod 755 "$pkg/DEBIAN/postinst"

mkdir -p "$out"
debfile="$out/esp32bench_${version}-1.${ID}${VERSION_ID}_amd64.deb"
dpkg-deb --root-owner-group --build "$pkg" "$debfile"
echo "built $debfile"
