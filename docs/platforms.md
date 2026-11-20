# macOS and Windows (phase 6)

The owner made macOS and Windows target platforms on 2026-11-19 (`docs/PLAN.md`, phase 6). Since 2026-11-20 both build in CI, as packages that are installed and smoke-tested on each system ([below](#where-it-stands)). The spike first asked one question first: does lcgamboa's QEMU fork, with esp32bench's patches, build on them, and do the simulator's hooks work there? It ran on GitHub's macOS (Apple silicon) and Windows runners (`.github/workflows/spike-platforms.yml`, run by hand), against test firmware built with ESP-IDF v6.0.2 in Espressif's container.

## What the spike found

| | macOS 15, arm64 (Homebrew) | Windows Server 2025, x86-64 (MSYS2 UCRT64) |
| --- | --- | --- |
| Patches 0001-0016 apply | Yes | Yes, once git stops converting line endings (`core.autocrlf false` before both checkouts) |
| Fork builds | Yes | Yes, statically (`--static`); see below |
| Firmware boots on the executables | not tried (the library is what the app uses) | Yes: the hello firmware on the ESP32, S3 and C3 |
| Library links | Yes, `libqemu-xtensa.dylib` (20 MB) and `libqemu-riscv32.dylib` (17 MB), with `tests/platforms/link-library.sh` | Yes, statically: `libqemu-xtensa.dll` (73 MB) and `libqemu-riscv32.dll` (76 MB), needing nothing but Windows' own DLLs |
| Hooks tests (`esp32bench-qemu-host`) | All 31 pass; 28 ran against the `.dylib`s, the other 3 need the fork's executables, which macOS does not ship | the host builds now (below); run by the Windows workflow |
| Core tests | 234 pass; the failures were tests that assumed Linux (`.so` names, `/proc`), since fixed, and light-sleep timing (below) | builds now (below) |

Problems found and how they were solved:

- **The fork's library script does not link on macOS.** `build_libqemu-esp32.sh` deletes the emulator, relinks it and reads the link command from ninja's output; on the runner it read nothing and ran an empty command, without failing. `tests/platforms/link-library.sh` asks ninja for the command instead (`ninja -t commands`, which rebuilds nothing), drops `main()`'s object and links a shared library. On Windows ninja keeps the arguments in a response file it deletes after the build, so the script relinks once with `-d keeprsp` and rewrites that file.
- **Windows: glib defined twice.** The fork asks for libslirp statically (`static: true` in `meson.build`), and on MSYS2 that brings glib's static library in beside its import library, so both emulators failed to link. `--static` links everything statically, which also makes the library self-contained: no MSYS2 DLLs to ship beside it.
- **Core, macOS.** `udev` (hotplug) and `prctl(PR_SET_PDEATHSIG)` (QEMU dies with the core) are Linux-only. The device crate now polls the port list every second elsewhere, and QEMU is stopped by `kill_on_drop` alone there. The whole workspace, tests included, type-checks for `aarch64-apple-darwin`.
- **macOS pseudo-terminals** reject the `IOSSIOSPEED` ioctl that `serialport` sets every baud rate with ("Not a typewriter"); USB serial adapters accept it. The pty-based serial tests are ignored on macOS.
- **Windows DLL exports.** QEMU marks a few symbols `dllexport` (its plugin API), which stops MinGW exporting the rest automatically, so the first DLLs exported none of what the host looks up. `link-library.sh` links with `--export-all-symbols`, and the Windows workflow checks the exports.
- **Light sleep on a shared runner.** On macOS every chip sleeps and wakes on its timer, but a busy runner overslept by up to 60 ms against the boot test's 10 ms (simulated time follows the host's clock without `-icount`). With `-icount` the S3 and C3 hold the window and the ESP32 does not (544 ms, then 477 ms); that is open, and not a porting problem.

## How the core runs on Windows

Type-checking the workspace for `x86_64-pc-windows-gnu` listed what Windows lacked; the whole workspace now passes clippy for Windows and macOS as well as Linux.

- **The core's socket** is a named pipe on Windows (`esp32bench_rpc::transport`): `\\.\pipe\esp32bench-<user>-core` by default, which is what Qt's `QLocalSocket` connects to, so the UI needed no protocol change. Creating the pipe's first instance fails while another core has it, which is the single-instance check the lock file is on Unix. Callers keep using paths; on Windows a path becomes a pipe name.
- **The simulator's links** (QEMU's UART and QMP, the C3's USB serial port, the host's pin link) are loopback TCP ports on Windows (`esp32bench_sim::link`), picked when the session starts. Unlike a Unix socket in a private directory, another user of the same computer could reach them while a simulation runs; named pipes (QEMU's `pipe:` chardev) would close that gap, and are the next step there.
- **The host** loads the DLL with `LoadLibraryW` and `GetProcAddress`, without a new dependency.
- **Process trees.** Builds and the simulator build are stopped with `taskkill /T /F` when cancelled (Windows has no process groups).
- **Session directories** live in `%TEMP%`, which is the user's own. A session whose QEMU has ended is cleaned up because Windows will then let its flash image be deleted; a running QEMU keeps it open. (On macOS the cleanup had a latent bug: it asked `/proc`, which macOS does not have, whether a session's owner still ran, and would have removed other running cores' sessions; it now uses `kill(pid, 0)`.)
- **The UI** starts `esp32benchd.exe` without a console window (`CREATE_NO_WINDOW`), so the simulator processes it starts have none either, and builds with MSVC.
- **Data folders.** The core found its data folder through `HOME`, which a Windows program started normally does not have. `esp32bench_distro::dirs` gives each system's own: `%LOCALAPPDATA%\esp32bench` (cache in `…\cache`) on Windows, `~/Library/Application Support` and `~/Library/Caches` on macOS, the XDG folders on Linux; and arduino-cli's own data folder on each (`%LOCALAPPDATA%\Arduino15`, `~/Library/Arduino15`, `~/.arduino15`), where the core reads installed cores and the debugger's GDB.
- **Consent tokens** came from `/dev/urandom`, which Windows does not have, so every build or flash of a downloaded project failed before asking. They come from the system's random source (`getrandom`).
- **Program names** need `.exe` on Windows: the host and the GDBs are looked up with the system's suffix.
- **The user's own ESP-IDF projects.** On Linux and macOS their builds source ESP-IDF's `export.sh` in bash. Windows has no POSIX shell, so there the core does what ESP-IDF's activation does, with no shell: ESP-IDF's Python runs `idf_tools.py export --format key-value`, and `<python> tools\idf.py` runs with what it printed. Espressif's installation manager (EIM) lays its tools out where `idf_tools.py` does not look (`C:\esp\<tool>\<version>` in CI), so for an install EIM recorded the environment comes from EIM's own activation script instead (`powershell -File <its PowerShell profile> -e` prints it) and its recorded Python. ESP-IDF is found at `IDF_PATH`, in EIM's record (`C:\Espressif\tools\eim_idf.json`; EIM 3.0 can leave `idfSelectedId` naming none of its installs), at `%USERPROFILE%\esp\esp-idf` (Espressif's guide) or in the classic installer's `C:\Espressif\frameworks`. `idf.py` and `idf_tools.py` refuse to run when `MSYSTEM` is set ("MSys/Mingw is not supported"), which Git Bash sets, so the core drops it for them. Both installs are checked in CI: `install.ps1` (the smoke test builds a project from a folder with a space in its name and boots it) and EIM through Espressif's action (whose own deactivate script failed after installing, so that step may fail).
- **Not checked yet:** the serial monitor and flashing over COM ports with a real board (CP210x and CH340 boards need their maker's driver, which the toolchain status says).

## Decision: the simulator ships prebuilt on macOS and Windows

On Linux the setup step builds the fork from source (about 10 minutes, with the distribution's packages). On macOS that needs Homebrew and the same build; on Windows it needs MSYS2 and a static build that took about 30 minutes on the runner. Neither is something to ask of a user. So on macOS and Windows the release workflow builds the patched fork (recipe-pinned, as on Linux) and the app ships its library: the simulator works right after install, with no build step. The source stays available as the GPL requires: the pinned commit, the patches in `core/crates/sim/patches/` and the build recipe are in this repository, and each release links them. Linux keeps building from source, where the toolchain is a package install away.

## Where it stands

Both platforms have packages built, installed and smoke-tested in CI on every change to the core, the UI or their packaging:

- **macOS** (`.github/workflows/macos.yml`, `packaging/macos/`): `ESP32 Bench.app` in a `.dmg` for Apple silicon, with the simulator in `Contents/Resources/lcgamboa` and the Homebrew libraries it needs beside it (`dylibbundler`, then each bundled library pointed at its neighbours and duplicate rpaths removed, which dyld refuses), `esp32bench://` registered, signed ad hoc (Apple silicon runs only signed code). Qt 6.10: 6.8's CMake files link Apple's AGL framework, which current SDKs no longer have.
- **Windows** (`.github/workflows/windows.yml`, `packaging/windows/`): a `.zip` and an Inno Setup installer that installs for the user without administrator rights and registers `esp32bench://`, with the simulator in `lcgamboa\`. Built with MSVC (in PowerShell: Git Bash's `/usr/bin/link` shadows MSVC's linker) and Qt 6.8.

The UI's 16 tests pass on both, including the integration test, which starts the real core, builds a project after asking for consent and runs it in the simulator. Each smoke test installs the package as a user gets it, then:

- starts the core and checks it over its socket (macOS) or named pipe (Windows), and that `toolchain.status` finds the bundled simulator;
- boots the hello firmware on the ESP32, S3 and C3 through it;
- drives pins both ways over the pin link (macOS: a GPIO trace; Windows: GPIO 2's toggling reaches the core and GPIO 4 driven from the core reaches the firmware);
- installs the pinned arduino-cli and the ESP32 core with the app's own toolchain install, builds the library's `blink` project, and boots the build in the simulator;
- on Windows, builds the user's own ESP-IDF project with ESP-IDF installed as Espressif's guide does, and boots it (and a second job builds one with an EIM install);
- keeps the UI up headless.

What is left:

1. Real boards, which needs hardware: the serial monitor, flashing and the automatic reset over USB serial on both.
2. Named pipes for the simulator's links on Windows.
3. A release with the `.dmg`, `.zip` and installer (a version tag's `packages.yml` run now builds and attaches them; a manual run gathers them without uploading), and the site's /desktop/ page offering them.
4. Signing: unsigned until the owner decides (macOS Gatekeeper asks users to allow an app from the internet; Windows SmartScreen warns).
5. An Intel Mac build (the `.dmg` is arm64 only) and Windows on Arm, if wanted.
