---
paths:
  - "packaging/**"
---

# Packaging rules

- Target is Arch Linux. Follow the Arch package guidelines: install under `/usr`, no files in `/usr/local` or `/opt` for our own binaries.
- Runtime dependencies must come from the official repos. AUR packages may only be `optdepends`, with a description of what they enable.
- Do not depend on the AUR package `qemu-esp-xtensa-git`. It is stale. QEMU is fetched or built by the app's own setup step.
- The PKGBUILD builds both parts: the Rust core with Cargo (`--locked`, `--release`) and the UI with CMake.
- The package installs the `.desktop` file (with the `x-scheme-handler/esp32bench` MIME type), the icon, and the udev rule under `/usr/lib/udev/rules.d/`.
- The udev rule grants access through `uaccess` and the `uucp` group. It must not make devices world-writable.
- Validate every PKGBUILD change with `namcap` and a clean build with `makepkg -s --cleanbuild` before finishing.
