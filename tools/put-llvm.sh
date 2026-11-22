#!/bin/sh
# put-llvm.sh - puts llvm-tblgen on a TaajFS disk, with what it reads to
# write the value types and the intrinsics, so it can be tried by hand:
#
#   /llvm-tblgen                       the program
#   /src/llvm/include/llvm/...         the .td files its two jobs read
#   /bad.td                            a file with a mistake in it
#
#   usage: put-llvm.sh DISK BIN_DIR
#
# BIN_DIR is where tools/build-llvm.py put llvm-tblgen for the disk's
# machine. LLVM_TBLGEN in the environment is the host's llvm-tblgen: its
# -d lists say which .td files the jobs read (25 of them, 1.9 MB). A file
# already on the disk is replaced.

set -eu
DISK=$1
BIN_DIR=$2
ROOT=$(cd "$(dirname "$0")/.." && pwd)
TFSTOOL=${TFSTOOL:-$ROOT/tools/tfstool-rs/target/release/tfstool}
TBLGEN=${LLVM_TBLGEN:-llvm-tblgen}
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

put llvm-tblgen "$BIN_DIR/llvm-tblgen"

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
echo "llvm-tblgen and $(sort -u "$WORK/sources" | wc -l) .td files are on $DISK"
