//! The pinned QEMU build. Change these together, and only to a tested release.
//!
//! Source: <https://github.com/espressif/qemu/releases>. Espressif does not ship a
//! checksum file; the hashes below match the asset digests GitHub records for the release.

use crate::Arch;

pub const QEMU_RELEASE: &str = "esp-develop-9.2.2-20261109";

pub struct QemuAsset {
    pub arch: Arch,
    pub url: &'static str,
    pub sha256: &'static str,
}

pub const QEMU_ASSETS: [QemuAsset; 2] = [
    QemuAsset {
        arch: Arch::Xtensa,
        url: "https://github.com/espressif/qemu/releases/download/esp-develop-9.2.2-20261109/qemu-xtensa-softmmu-esp_develop_9.2.2_20261109-x86_64-linux-gnu.tar.xz",
        sha256: "fbc3bffa9fa71192847969a627a584aabe2e86d85bed4902a096ff06c2b8a815",
    },
    QemuAsset {
        arch: Arch::Riscv32,
        url: "https://github.com/espressif/qemu/releases/download/esp-develop-9.2.2-20261109/qemu-riscv32-softmmu-esp_develop_9.2.2_20261109-x86_64-linux-gnu.tar.xz",
        sha256: "afa59465495295b8aa4f804768983fa30b0bbf33d0f68a047f9caa436b9164c0",
    },
];

pub fn asset(arch: Arch) -> &'static QemuAsset {
    match arch {
        Arch::Xtensa => &QEMU_ASSETS[0],
        Arch::Riscv32 => &QEMU_ASSETS[1],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assets_are_pinned_to_the_release() {
        for arch in Arch::ALL {
            let a = asset(arch);
            assert_eq!(a.arch, arch);
            assert!(a.url.contains(QEMU_RELEASE));
            assert!(a.url.contains(&format!("qemu-{arch}-softmmu")));
            assert!(a.url.starts_with("https://"));
            assert_eq!(a.sha256.len(), 64);
        }
    }
}
