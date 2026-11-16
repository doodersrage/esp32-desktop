---
name: arch-package
description: Create, update or test the Arch Linux packaging for this project. Use when editing a PKGBUILD, the .desktop file or the udev rule, when preparing an AUR release, or when a dependency changes.
---

# Arch packaging

Packaging files are in `packaging/`. There are two packages: a `-git` package that builds from the repo, and a `-bin` package added once tagged releases exist.

## Updating the PKGBUILD

1. Edit `packaging/<pkgname>/PKGBUILD`.
2. Keep `depends` to official-repo packages. Put anything from the AUR in `optdepends` with a reason.
3. Build the core with `cargo build --release --locked` and the UI with CMake, both inside `build()`.
4. In `package()`, install:
    - the core daemon and CLI, and the UI binary, to `/usr/bin`
    - the `.desktop` file to `/usr/share/applications`
    - the icon to `/usr/share/icons/hicolor/`
    - the udev rule to `/usr/lib/udev/rules.d/`
5. Regenerate metadata: `makepkg --printsrcinfo > .SRCINFO`.

## Testing

Run all of these before finishing:

```sh
makepkg -s --cleanbuild
namcap PKGBUILD
namcap *.pkg.tar.zst
```

For a dependency check that does not rely on what is already installed, build in a clean chroot with `extra-x86_64-build` from the `devtools` package.

After installing the package, check:

- The app starts from the Plasma launcher.
- `xdg-mime query default x-scheme-handler/esp32bench` returns the app's `.desktop` file.
- A connected board's serial port is readable without root after `udevadm control --reload` and replugging.

## .desktop file

- Must include `MimeType=x-scheme-handler/esp32bench;` and an `Exec` line ending in `%u`.
- Validate with `desktop-file-validate`.

## udev rule

- Match Espressif's USB vendor id and the common USB-serial bridges used on ESP32 boards.
- Grant access with `TAG+="uaccess"` and `GROUP="uucp"`. Never set `MODE="0666"`.

## AUR release

Only push to the AUR when the owner asks. Bump `pkgver` or `pkgrel`, regenerate `.SRCINFO`, and commit both files together.
