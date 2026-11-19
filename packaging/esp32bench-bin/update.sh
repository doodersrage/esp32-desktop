#!/bin/sh
# Point this PKGBUILD at release v<version>: set pkgver (pkgrel back to 1), take the
# SHA-256 of the tarball packages.yml attached to the release, and regenerate
# .SRCINFO. The release step after the tag's Packages run has passed.
#
# Usage: packaging/esp32bench-bin/update.sh 0.1.6
set -eu

version=${1:?usage: update.sh <version>, e.g. 0.1.6}
case $version in
  *[!0-9.]* | .* | *. | *..*) echo "update.sh: not a version: $version" >&2; exit 2 ;;
esac

dir=$(cd "$(dirname "$0")" && pwd)
url="https://github.com/doodersrage/esp32-desktop/releases/download/v$version/esp32bench-$version-x86_64.tar.gz"
tmp=$(mktemp -d)
trap 'rm -rf "${tmp:?}"' EXIT

curl -fsSL --proto '=https' -o "$tmp/release.tar.gz" "$url"
sum=$(sha256sum "$tmp/release.tar.gz" | cut -d' ' -f1)

sed -i \
  -e "s/^pkgver=.*/pkgver=$version/" \
  -e "s/^pkgrel=.*/pkgrel=1/" \
  -e "s/^sha256sums=.*/sha256sums=('$sum')/" \
  "$dir/PKGBUILD"
(cd "$dir" && makepkg --printsrcinfo > .SRCINFO)

echo "esp32bench-bin $version-1: sha256 $sum"
