#!/bin/sh
# Phase 6 spike: link lcgamboa's QEMU as a library from a finished build, by taking
# ninja's own link command for the emulator (`ninja -t commands`, which rebuilds
# nothing), dropping main()'s object, and linking a shared library instead:
# libqemu-<arch>.dylib on macOS, libqemu-<arch>.dll on Windows (MSYS2). The fork's
# build_libqemu-esp32.sh does this by relinking and capturing ninja's output, which
# on macOS captured no command.
#
# Usage: link-library.sh <build dir> <arch: xtensa|riscv32>
set -eu
build=${1:?build dir}
arch=${2:?arch}
cd "$build"
case "$(uname -s)" in
  Darwin) target="qemu-system-$arch-unsigned"; out="libqemu-$arch.dylib"; kind="-dynamiclib" ;;
  # Export everything: QEMU marks a few symbols dllexport (its plugin API), which
  # stops MinGW exporting the rest automatically, qemu_init and the hooks included.
  MINGW*|MSYS*) target="qemu-system-$arch.exe"; out="libqemu-$arch.dll"; kind="-shared -Wl,--export-all-symbols" ;;
  *) target="qemu-system-$arch"; out="libqemu-$arch.so"; kind="-shared" ;;
esac
# main()'s object: system_main.c.o, or .c.obj on Windows.
main_obj="$target.p/system_main.c.o[bj]*"
cmd=$(ninja -t commands "$target" | tail -n 1)
case $cmd in
  *"@$target.rsp"*)
    # Windows: the arguments are in a response file, which ninja deletes after
    # linking; relink once keeping it, then rewrite the file rather than the command.
    [ -f "$target.rsp" ] || { rm -f "$target"; ninja -d keeprsp "$target" > /dev/null; }
    sed -e "s|$main_obj||" -e "s|-o $target|$kind -o $out|" -e "s|-o \"$target\"|$kind -o $out|" \
      "$target.rsp" > "$out.rsp"
    grep -q -- "-o $out" "$out.rsp" || { echo "link-library.sh: no -o $target in $target.rsp" >&2; exit 1; }
    cmd=$(printf '%s' "$cmd" | sed -e "s|@$target.rsp|@$out.rsp|")
    ;;
  *" -o $target "*)
    cmd=$(printf '%s' "$cmd" | sed -e "s| $main_obj | |" -e "s| -o $target | $kind -o $out |")
    ;;
  *) echo "link-library.sh: no link command for $target: $cmd" >&2; exit 1 ;;
esac
echo "linking $out"
eval "$cmd"
ls -la "$out"
