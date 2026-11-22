# clangd in your own editor

ESP32 Bench has no editor of its own (`PLAN.md`, scope for v1). It can set up [clangd](https://clangd.llvm.org/), the C and C++ language server, so the editor you use (Kate, VS Code, Neovim, CLion and others) completes, checks and navigates the code of your own projects: ESP-IDF projects and Arduino sketches you opened with File → Open Project Folder.

## What the app does

1. Build the project once for a board. Every build of your own project leaves a compilation database (`compile_commands.json`) in the app's cache: an ESP-IDF build's own, or, for a sketch, arduino-cli's mapped back from its working copy to your files, your `.ino` included (compiled as C++ with `Arduino.h`, as arduino-cli does).
2. Edit → Set up clangd for this board… (or `esp32bench clangd <project> --board <board>`). After asking, the app writes `.clangd` into the project folder. It points clangd at that build's database and removes the GCC options clangd does not know (`-mlongcalls` and the other Xtensa ones, `-fstrict-volatile-bitfields`, `-fno-tree-switch-conversion`). A `.clangd` you wrote yourself is never replaced. Switching boards: set it up again for the other board.

## The one step in your editor

clangd must be allowed to ask the ESP32 cross-compilers for their target and headers. That is clangd's `--query-driver` option, and it belongs in your editor's settings, not the project. The app shows the exact option when it sets clangd up; it looks like this:

- Arduino sketches: `--query-driver=/home/<you>/.arduino15/packages/esp32/tools/**/bin/*`
- ESP-IDF projects: `--query-driver=/home/<you>/.espressif/tools/**/bin/*`

### Kate

Settings → Configure Kate → LSP Client → User Server Settings, then add `--query-driver` to clangd's command for C and C++:

```json
{
  "servers": {
    "c": {
      "command": ["clangd", "-log=error", "--background-index", "--query-driver=/home/<you>/.arduino15/packages/esp32/tools/**/bin/*"]
    },
    "cpp": {
      "command": ["clangd", "-log=error", "--background-index", "--query-driver=/home/<you>/.arduino15/packages/esp32/tools/**/bin/*"]
    }
  }
}
```

For an ESP-IDF project, use its query driver instead, or list both separated by a comma. Kate opens `.ino` files as C++ once their highlighting mode is C++ (Tools → Mode).

### VS Code

Install the clangd extension (`llvm-vs-code-extensions.vscode-clangd`); when it offers to turn off Microsoft's C/C++ IntelliSense, accept. Then in `settings.json`:

```json
{
  "clangd.arguments": ["--query-driver=/home/<you>/.arduino15/packages/esp32/tools/**/bin/*"],
  "files.associations": { "*.ino": "cpp" }
}
```

## Checked

With clangd 23 (2026-11-22), `clangd --check` on a sketch's own `.ino`, run in its folder with only the `.clangd` and `--query-driver`, completes with no errors for the ESP32-C3 (RISC-V). On the ESP32 (Xtensa) the sketch itself checks clean; indexing picolibc's `limits.h` on its own reports one error (`__GLIBC_USE` used before picolibc defines it for clang), which does not affect your code. clangd still logs that it ignores the GCC-only options: it reads them before applying `.clangd`.
