#!/bin/bash
# Build esp32bench's Arch release files inside an archlinux container: the tarball
# (esp32bench-<version>-x86_64.tar.gz, what esp32bench-bin installs) and the
# esp32bench-bin package made from it.
#
# Usage: build.sh <repo, read-only is fine> <output dir>
set -euo pipefail

repo=$1
out=$2

pacman -Syu --noconfirm --needed base-devel cargo cmake ninja git qt6-base qt6-svg qt6-tools \
  systemd-libs hicolor-icon-theme > /dev/null

work=$(mktemp -d)
chmod 755 "$work"
# The repository's files only (tracked, or new and not ignored): no build output.
mkdir "$work/src"
git -c safe.directory='*' -C "$repo" ls-files -z --cached --others --exclude-standard \
  | (cd "$repo" && tar --null --files-from=- -cf -) | tar -xf - -C "$work/src"
cd "$work/src"
export CARGO_TARGET_DIR="$work/target"
cargo build --manifest-path core/Cargo.toml --locked --release --bins
cargo test --manifest-path core/Cargo.toml --locked --release --workspace --lib
cmake -S ui -B "$work/build-ui" -G Ninja -DCMAKE_BUILD_TYPE=None \
  -DCMAKE_INSTALL_PREFIX=/usr -DBUILD_TESTING=OFF > /dev/null
cmake --build "$work/build-ui"

version=$(sed -n 's/^version = "\(.*\)"/\1/p' core/Cargo.toml | head -1)
name="esp32bench-$version-x86_64"
sh packaging/install-files.sh "$PWD" "$CARGO_TARGET_DIR" "$work/build-ui" "$work/$name" uucp
cp LICENSE README.md "$work/$name/"
mkdir -p "$out"
tar --owner=0 --group=0 -C "$work" -czf "$out/$name.tar.gz" "$name"

# esp32bench-bin from that tarball: its PKGBUILD names the release URL, so put the
# file where makepkg looks first and give the PKGBUILD this tarball's checksum.
bin="$work/bin"
mkdir "$bin"
sed -e "s/^pkgver=.*/pkgver=$version/" \
    -e "s/^sha256sums=.*/sha256sums=('$(sha256sum "$out/$name.tar.gz" | cut -d' ' -f1)')/" \
    packaging/esp32bench-bin/PKGBUILD > "$bin/PKGBUILD"
# No -debug package beside it (the image's makepkg.conf makes one by default).
echo "options=('!debug')" >> "$bin/PKGBUILD"
cp "$out/$name.tar.gz" "$bin/"
useradd -m builder 2> /dev/null || true
chown -R builder "$bin"
su builder -c "cd '$bin' && makepkg --nodeps --noconfirm > makepkg.log 2>&1" \
  || { cat "$bin/makepkg.log"; exit 1; }
cp "$bin"/esp32bench-bin-*.pkg.tar.zst "$out/"
ls "$out/$name.tar.gz" "$out"/esp32bench-bin-*.pkg.tar.zst
