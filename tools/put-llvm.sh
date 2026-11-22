#!/bin/sh
# put-llvm.sh - puts LLVM's tools on a TaajFS disk, with something for
# each to work on, so they can be tried by hand:
#
#   /llvm-tblgen                       the programs
#   /llc
#   /llvm-mc
#   /src/llvm/include/llvm/...         the .td files llvm-tblgen's two
#                                      jobs read
#   /bad.td                            a file with a mistake in it
#   /x86_64.ll, /aarch64.ll            libc/programs/cwords.c in LLVM's
#                                      own language, for each machine,
#                                      for llc
#   /aarch64.s                         and in AArch64 assembly language,
#                                      for llvm-mc
#   /bad.ll, /bad.s                    a mistake each
#
#   usage: put-llvm.sh DISK BIN_DIR
#
# BIN_DIR is where tools/build-llvm.py put the tools for the disk's
# machine. LLVM_TBLGEN in the environment is the host's llvm-tblgen: its
# -d lists say which .td files the jobs read (25 of them, 1.9 MB). The
# .ll and .s files are written by the host's clang and llc from the same
# directory. A file already on the disk is replaced. The tools take 49 MB
# (the PC's) or 43 MB (AArch64's): the disk must be big enough.

set -eu
DISK=$1
BIN_DIR=$2
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

for tool in llvm-tblgen llc llvm-mc; do
    put $tool "$BIN_DIR/$tool"
done

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
echo "llvm-tblgen, llc, llvm-mc, $(sort -u "$WORK/sources" | wc -l) .td files and cwords.c for llc and llvm-mc are on $DISK"
