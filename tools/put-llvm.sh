#!/bin/sh
# put-llvm.sh - puts LLVM's tools on a TaajFS disk, with something for
# each to work on, so they can be tried by hand:
#
#   /llvm-tblgen                       the programs
#   /llc
#   /llvm-mc
#   /clang
#   /ld.lld                            lld (it picks the linker by name)
#   /lib/clang/18/include/...          the headers clang brings with it
#                                      (all but the few whose names are
#                                      longer than TaajFS takes)
#   /include/...                       the C library's headers
#   /libc/programs/*.c                 TheTaaJ's C programs, for clang,
#                                      under the names the build uses:
#                                        clang @cflags -c libc/programs/chello.c
#   /cflags                            the options TheTaaJ's build
#                                      compiles them with, for this
#                                      machine, as a response file
#   /lib/libc.a, /user.ld, /ldflags    the C library, the linker script
#                                      and the build's linker options:
#                                        ld.lld @ldflags chello.o lib/libc.a -o chello2
#   /lflags                            the same, as clang takes them when
#                                      it links, starting ld.lld itself:
#                                        with programs clang @cflags @lflags libc/programs/chello.c lib/libc.a -o chello3
#   /tmp                               where clang puts the object it
#                                      compiles before it links
#   /src/llvm/include/llvm/...         the .td files llvm-tblgen's two
#                                      jobs read
#   /bad.td                            a file with a mistake in it
#   /x86_64.ll, /aarch64.ll            libc/programs/cwords.c in LLVM's
#                                      own language, for each machine,
#                                      for llc
#   /aarch64.s                         and in AArch64 assembly language,
#                                      for llvm-mc
#   /bad.ll, /bad.s, /bad.c            a mistake each
#
#   usage: put-llvm.sh DISK BIN_DIR C_BIN
#
# BIN_DIR is where tools/build-llvm.py put the tools for the disk's
# machine. LLVM_TBLGEN in the environment is the host's llvm-tblgen: its
# -d lists say which .td files the jobs read (25 of them, 1.9 MB). The
# .ll and .s files are written by the host's clang and llc from the same
# directory. A file already on the disk is replaced. The tools take
# 131 MB (the PC's) or 110 MB (AArch64's), clang 82 or 67 of it, and its
# headers 13 MB more, and ld.lld 46 or 39: the disk must be big enough.
# BIN_DIR/../lib holds clang's headers; C_BIN is where the build put this
# machine's libc.a.

set -eu
DISK=$1
BIN_DIR=$2
C_BIN=$3
ROOT=$(cd "$(dirname "$0")/.." && pwd)
TFSTOOL=${TFSTOOL:-$ROOT/tools/tfstool-rs/target/release/tfstool}
TBLGEN=${LLVM_TBLGEN:-llvm-tblgen}
HOST_BIN=$(dirname "$(command -v "$TBLGEN")")
LLVM=$ROOT/ports/llvm/llvm
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

put() {
    "$TFSTOOL" rm "$DISK" "$1" > /dev/null 2>&1 || true
    "$TFSTOOL" put "$DISK" "$1" "$2" > /dev/null || { echo "put-llvm: cannot put $1 on $DISK" >&2; exit 1; }
}
directory() {
    "$TFSTOOL" mkdir "$DISK" "$1" > /dev/null 2>&1 || true
}

for tool in llvm-tblgen llc llvm-mc clang; do
    put $tool "$BIN_DIR/$tool"
done
put ld.lld "$BIN_DIR/lld"

# clang's headers, the C library's, and TheTaaJ's C programs.
put_tree() { # put_tree FROM TO
    for d in $(cd "$1" && find . -type d | sort); do
        d=${d#./}
        [ "$d" = . ] || directory "$2/$d"
    done
    for f in $(cd "$1" && find . -type f | sort); do
        f=${f#./}
        # TaajFS names are at most 64 bytes: all of clang's headers fit
        # (before version 5, 27 did not leave room for CUDA's and HIP's).
        [ "$(basename "$f" | wc -c)" -gt 65 ] || put "$2/$f" "$1/$f"
    done
}
for d in lib lib/clang lib/clang/18 lib/clang/18/include include libc libc/programs tmp; do
    directory $d
done
put_tree "$BIN_DIR/../lib/clang/18/include" lib/clang/18/include
put_tree "$ROOT/libc/include" include
for f in "$ROOT"/libc/programs/*.c; do
    put "libc/programs/$(basename "$f")" "$f"
done
put lib/libc.a "$C_BIN/libc.a"
put user.ld "$ROOT/user/user.ld"
case $(file -b "$BIN_DIR/clang") in
    *aarch64*) echo "--target=aarch64-unknown-none-elf -std=c11 -O2 -Wall -Wextra -Werror -ffreestanding" \
                    "-fno-stack-protector -fno-pic -nostdlibinc -isystem /include" > "$WORK/cflags" ;;
    *) echo "--target=x86_64-unknown-none-elf -std=c11 -O2 -Wall -Wextra -Werror -ffreestanding" \
            "-fno-stack-protector -fno-pic -mno-red-zone -nostdlibinc -isystem /include" > "$WORK/cflags" ;;
esac
put cflags "$WORK/cflags"
echo "-static -nostdlib --gc-sections -s -T user.ld" > "$WORK/ldflags"
case $(file -b "$BIN_DIR/clang") in *aarch64*) echo "-z max-page-size=4096" >> "$WORK/ldflags" ;; esac
put ldflags "$WORK/ldflags"
echo "-static -nostdlib -Wl,--gc-sections -s -T user.ld" > "$WORK/lflags"
case $(file -b "$BIN_DIR/clang") in *aarch64*) echo "-Wl,-z,max-page-size=4096" >> "$WORK/lflags" ;; esac
put lflags "$WORK/lflags"

# The inputs, and what the host's llvm-tblgen says they read.
for job in "include/llvm/CodeGen/ValueTypes.td -gen-vt" "include/llvm/IR/Intrinsics.td -gen-intrinsic-enums"; do
    set -- $job
    "$TBLGEN" "$2" -I "$LLVM/include" "$LLVM/$1" -o "$WORK/out" -d "$WORK/out.d"
    { echo "$LLVM/$1"; tr ' ' '\n' < "$WORK/out.d" | grep '\.td$' || true; } >> "$WORK/sources"
done
directory src
directory src/llvm
for file in $(sort -u "$WORK/sources" | sed "s|^$LLVM/||"); do
    path=src/llvm
    for part in $(dirname "$file" | tr '/' ' '); do
        path=$path/$part
        directory "$path"
    done
    put "src/llvm/$file" "$LLVM/$file"
done

printf 'class Thing;\ndef something : Nothing;\n' > "$WORK/bad.td"
put bad.td "$WORK/bad.td"

# What llc and llvm-mc work on.
for machine in x86_64 aarch64; do
    "$HOST_BIN/clang" --target=$machine-unknown-none-elf -O2 -ffreestanding -nostdlibinc -isystem "$ROOT/libc/include" \
        -S -emit-llvm "$ROOT/libc/programs/cwords.c" -o "$WORK/$machine.ll" \
        || { echo "put-llvm: the host's clang failed on cwords.c" >&2; exit 1; }
    put $machine.ll "$WORK/$machine.ll"
done
"$HOST_BIN/llc" -O2 "$WORK/aarch64.ll" -o "$WORK/aarch64.s" || { echo "put-llvm: the host's llc failed" >&2; exit 1; }
put aarch64.s "$WORK/aarch64.s"
printf 'define i32 @f() {\n  ret i64 0\n}\n' > "$WORK/bad.ll"
printf '\tmovq %%rax, %%rbx\n\tnotaninstruction %%rax\n' > "$WORK/bad.s"
put bad.ll "$WORK/bad.ll"
put bad.s "$WORK/bad.s"
printf 'int f(void) {\n    return x;\n}\n' > "$WORK/bad.c"
put bad.c "$WORK/bad.c"
echo "llvm-tblgen, llc, llvm-mc, clang, ld.lld, $(sort -u "$WORK/sources" | wc -l) .td files, cwords.c for llc and llvm-mc, and the C programs and libc.a for clang and ld.lld are on $DISK"
