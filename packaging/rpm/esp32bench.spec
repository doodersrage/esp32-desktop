# esp32bench for Fedora. Packages a tree staged by packaging/install-files.sh, which
# packaging/rpm/build.sh builds from source in the target release's container first.
# Library requirements are generated from the binaries.

%global debug_package %{nil}

Name:           esp32bench
Version:        %{esp32bench_version}
Release:        1%{?dist}
Summary:        ESP32 workbench and simulator
License:        GPL-2.0-or-later
URL:            https://esp32bench.dev/desktop/
ExclusiveArch:  x86_64

# Plugins Qt loads at run time are not found as libraries: the SVG icon plugin, the
# X11 platform plugin's cursor library, and the Wayland platform plugin (without it
# Qt falls back to X11 on a Wayland desktop).
Requires:       qt6-qtsvg
Requires:       xcb-util-cursor
Recommends:     qt6-qtwayland
Requires:       hicolor-icon-theme
Requires:       systemd-udev
# Downloads (arduino-cli, Espressif's QEMU) are fetched with curl and unpacked with tar.
Requires:       curl
Requires:       tar
Requires:       xz
# Building the simulator (lcgamboa's QEMU) on this computer.
Recommends:     git
Recommends:     gcc
Recommends:     make
Recommends:     pkgconf-pkg-config
Recommends:     ninja-build
Recommends:     python3
Recommends:     perl
Recommends:     glib2-devel
Recommends:     pixman-devel
Recommends:     libgcrypt-devel
Recommends:     libslirp-devel

%description
Browse and build the esp32bench.dev project library and your own ESP-IDF
projects, flash and monitor ESP32 boards, and run builds in a simulator with
virtual parts. The recommended packages build the simulator (lcgamboa's QEMU)
on this computer; arduino-cli is installed by the app.

%install
cp -a %{esp32bench_staged}/. %{buildroot}/

%post
if command -v udevadm > /dev/null; then
  udevadm control --reload-rules > /dev/null 2>&1 || :
  udevadm trigger --subsystem-match=tty > /dev/null 2>&1 || :
fi

%files
%license /usr/share/licenses/esp32bench/LICENSE
/usr/bin/esp32bench
/usr/bin/esp32benchd
/usr/bin/esp32bench-gui
/usr/lib/esp32bench/
/usr/lib/udev/rules.d/69-esp32bench.rules
/usr/share/applications/esp32bench.desktop
/usr/share/icons/hicolor/scalable/apps/esp32bench.svg

%changelog
* Thu Nov 19 2026 esp32bench <https://github.com/doodersrage/esp32-desktop/issues>
- Packaged for Fedora. Release notes: https://github.com/doodersrage/esp32-desktop/releases
