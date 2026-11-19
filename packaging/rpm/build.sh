#!/bin/bash
# Build esp32bench's .rpm inside a Fedora container (packaging/ci/build.sh runs it
# in each supported release), against that release's Qt and libraries.
#
# Usage: build.sh <repo, read-only is fine> <output dir>
set -euo pipefail

repo=$1
out=$2

dnf install -y -q gcc gcc-c++ cmake ninja-build pkgconf-pkg-config curl git tar \
  qt6-qtbase-devel qt6-qttools-devel qt6-linguist systemd-devel rpm-build > /dev/null

# Rust from rustup, the same toolchain the Debian and Ubuntu packages use.
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
sh packaging/install-files.sh "$PWD" "$CARGO_TARGET_DIR" "$work/build-ui" "$work/staged" dialout

rpmbuild -bb packaging/rpm/esp32bench.spec \
  --define "_topdir $work/rpmbuild" \
  --define "esp32bench_version $version" \
  --define "esp32bench_staged $work/staged" > /dev/null
mkdir -p "$out"
cp "$work"/rpmbuild/RPMS/x86_64/esp32bench-*.rpm "$out/"
ls "$out"/esp32bench-*.rpm
