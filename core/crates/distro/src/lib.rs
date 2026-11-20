//! The Linux distributions esp32bench supports (Arch, Debian and Ubuntu, Fedora), told
//! apart by `/etc/os-release`, and what differs between them: the names of the
//! packages a tool comes in, the command that installs them (shown to the user, never
//! run: no root, CLAUDE.md rule 5), and the group that owns serial ports.

use std::path::Path;

pub mod dirs;

/// A family of distributions that share package names and tools.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Distro {
    Arch,
    /// Debian, Ubuntu and their derivatives (apt).
    Debian,
    /// Fedora (dnf).
    Fedora,
    /// Anything else: hints name what is needed, not a command.
    Other,
}

/// Something the app needs from the system, by what it is rather than by any one
/// distribution's package name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Need {
    Git,
    Make,
    Gcc,
    PkgConfig,
    Ninja,
    Python,
    /// Python's venv module, which QEMU's configure uses (a separate package on Debian).
    PythonVenv,
    Perl,
    Sed,
    Glib,
    Pixman,
    Libgcrypt,
    Slirp,
    ArduinoCli,
}

impl Distro {
    /// The running system's family, from `/etc/os-release`.
    pub fn detect() -> Distro {
        Self::detect_at(Path::new("/etc/os-release"))
    }

    pub fn detect_at(os_release: &Path) -> Distro {
        std::fs::read_to_string(os_release)
            .map(|s| Self::from_os_release(&s))
            .unwrap_or(Distro::Other)
    }

    /// From the text of an os-release file: `ID`, then `ID_LIKE`.
    pub fn from_os_release(text: &str) -> Distro {
        let field = |key: &str| -> Vec<String> {
            text.lines()
                .filter_map(|l| l.strip_prefix(key)?.strip_prefix('='))
                .flat_map(|v| {
                    v.trim()
                        .trim_matches(|c| c == '"' || c == '\'')
                        .split_whitespace()
                        .map(str::to_ascii_lowercase)
                        .collect::<Vec<_>>()
                })
                .collect()
        };
        let classify = |id: &str| match id {
            "arch" | "archarm" | "endeavouros" | "manjaro" | "cachyos" | "garuda" => {
                Some(Distro::Arch)
            }
            "debian" | "ubuntu" | "linuxmint" | "pop" | "elementary" | "zorin" | "kali"
            | "raspbian" => Some(Distro::Debian),
            "fedora" => Some(Distro::Fedora),
            _ => None,
        };
        field("ID")
            .iter()
            .chain(field("ID_LIKE").iter())
            .find_map(|id| classify(id))
            .unwrap_or(Distro::Other)
    }

    pub fn name(self) -> &'static str {
        match self {
            Distro::Arch => "Arch Linux",
            Distro::Debian => "Debian or Ubuntu",
            Distro::Fedora => "Fedora",
            Distro::Other => "this distribution",
        }
    }

    /// The package that provides `need`, or None where there is no package for it
    /// (arduino-cli outside Arch: the app installs it itself).
    pub fn package(self, need: Need) -> Option<&'static str> {
        use Distro::*;
        use Need::*;
        Some(match (self, need) {
            (_, Git) => "git",
            (_, Make) => "make",
            (_, Gcc) => "gcc",
            (Arch | Other, PkgConfig) => "pkgconf",
            (Debian, PkgConfig) => "pkgconf",
            (Fedora, PkgConfig) => "pkgconf-pkg-config",
            (Arch | Other, Ninja) => "ninja",
            (Debian | Fedora, Ninja) => "ninja-build",
            (Arch | Other, Python) => "python",
            (Debian | Fedora, Python) => "python3",
            (Debian, PythonVenv) => "python3-venv",
            // Part of the python package elsewhere.
            (Arch | Fedora | Other, PythonVenv) => return None,
            (_, Perl) => "perl",
            (_, Sed) => "sed",
            (Arch | Other, Glib) => "glib2",
            (Debian, Glib) => "libglib2.0-dev",
            (Fedora, Glib) => "glib2-devel",
            (Arch | Other, Pixman) => "pixman",
            (Debian, Pixman) => "libpixman-1-dev",
            (Fedora, Pixman) => "pixman-devel",
            (Arch | Other, Libgcrypt) => "libgcrypt",
            (Debian, Libgcrypt) => "libgcrypt20-dev",
            (Fedora, Libgcrypt) => "libgcrypt-devel",
            (Arch | Other, Slirp) => "libslirp",
            (Debian, Slirp) => "libslirp-dev",
            (Fedora, Slirp) => "libslirp-devel",
            (Arch, ArduinoCli) => "arduino-cli",
            (Debian | Fedora | Other, ArduinoCli) => return None,
        })
    }

    /// The command that installs `packages`, or None on an unknown distribution.
    pub fn install_command(self, packages: &[&str]) -> Option<String> {
        if packages.is_empty() {
            return None;
        }
        let list = packages.join(" ");
        match self {
            Distro::Arch => Some(format!("sudo pacman -S --needed {list}")),
            Distro::Debian => Some(format!("sudo apt install {list}")),
            Distro::Fedora => Some(format!("sudo dnf install {list}")),
            Distro::Other => None,
        }
    }

    /// What to tell a user who needs `needs`: the command, or on an unknown
    /// distribution the packages by their usual names.
    pub fn install_hint(self, needs: &[Need]) -> Option<String> {
        let mut pkgs: Vec<&str> = needs.iter().filter_map(|n| self.package(*n)).collect();
        pkgs.dedup();
        if pkgs.is_empty() {
            return None;
        }
        Some(match self.install_command(&pkgs) {
            Some(cmd) => cmd,
            None => format!(
                "install these with your package manager: {}",
                pkgs.join(", ")
            ),
        })
    }

    /// The group that owns serial ports.
    pub fn serial_group(self) -> &'static str {
        match self {
            Distro::Arch => "uucp",
            Distro::Debian | Distro::Fedora | Distro::Other => "dialout",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tells_families_apart() {
        let os = |s: &str| Distro::from_os_release(s);
        assert_eq!(os("NAME=\"Arch Linux\"\nID=arch\n"), Distro::Arch);
        assert_eq!(os("ID=endeavouros\nID_LIKE=arch\n"), Distro::Arch);
        assert_eq!(os("ID=debian\nVERSION_ID=\"13\"\n"), Distro::Debian);
        assert_eq!(
            os("ID=ubuntu\nID_LIKE=debian\nVERSION_ID=\"26.04\"\n"),
            Distro::Debian
        );
        assert_eq!(os("ID=neon\nID_LIKE=\"ubuntu debian\"\n"), Distro::Debian);
        assert_eq!(os("ID=fedora\nVERSION_ID=45\n"), Distro::Fedora);
        assert_eq!(
            os("ID=opensuse-tumbleweed\nID_LIKE=\"opensuse suse\"\n"),
            Distro::Other
        );
        assert_eq!(os(""), Distro::Other);
        // VERSION_ID and the like do not count as ID.
        assert_eq!(os("ID_LIKE=debian\nIDX=fedora\n"), Distro::Debian);
    }

    #[test]
    fn hints_name_each_distributions_packages() {
        let needs = [Need::Ninja, Need::Slirp, Need::PythonVenv];
        assert_eq!(
            Distro::Arch.install_hint(&needs).unwrap(),
            "sudo pacman -S --needed ninja libslirp"
        );
        assert_eq!(
            Distro::Debian.install_hint(&needs).unwrap(),
            "sudo apt install ninja-build libslirp-dev python3-venv"
        );
        assert_eq!(
            Distro::Fedora.install_hint(&needs).unwrap(),
            "sudo dnf install ninja-build libslirp-devel"
        );
        assert!(
            Distro::Other
                .install_hint(&needs)
                .unwrap()
                .contains("ninja, libslirp")
        );
        assert_eq!(Distro::Debian.install_hint(&[]), None);
        assert_eq!(Distro::Fedora.package(Need::ArduinoCli), None);
        assert_eq!(Distro::Arch.serial_group(), "uucp");
        assert_eq!(Distro::Debian.serial_group(), "dialout");
    }
}
