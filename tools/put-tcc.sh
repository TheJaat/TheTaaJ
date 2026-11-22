#!/bin/sh
# put-tcc.sh - puts TCC and what it needs on a TaajFS disk, where tcc
# on TheTaaJ looks for it:
#
#   /tcc                 the compiler
#   /include/...         the C library's headers (libc/include)
#   /lib/libc.a          the C library
#   /lib/libc-rust.o     its Rust half, for building it on TheTaaJ
#   /lib/tcc/include/... TCC's own headers
#   /lib/tcc/libtcc1.a   TCC's helpers
#
#   usage: put-tcc.sh DISK TCC_DIR LIBC_A
#
# TCC_DIR is what tools/build-tcc.sh built, LIBC_A the C library for the
# same CPU. A file already on the disk is replaced, so this does for a
# disk that is kept from one run to the next too (make run-disk).

set -eu
DISK=$1
TCC_DIR=$2
LIBC_A=$3
ROOT=$(cd "$(dirname "$0")/.." && pwd)
TFSTOOL=${TFSTOOL:-$ROOT/tools/tfstool-rs/target/release/tfstool}

put() {
    "$TFSTOOL" rm "$DISK" "$1" > /dev/null 2>&1 || true
    "$TFSTOOL" put "$DISK" "$1" "$2" > /dev/null || { echo "put-tcc: cannot put $1 on $DISK" >&2; exit 1; }
}
# A directory that may be there already.
directory() {
    "$TFSTOOL" mkdir "$DISK" "$1" > /dev/null 2>&1 || true
}
# tree HOST_DIR DISK_DIR: every directory and file in it, by the same
# names.
tree() {
    directory "$2"
    for d in $(cd "$1" && find . -mindepth 1 -type d | sort); do directory "$2/${d#./}"; done
    for f in $(cd "$1" && find . -type f | sort); do put "$2/${f#./}" "$1/${f#./}"; done
}

put tcc "$TCC_DIR/tcc"
tree "$ROOT/libc/include" include
directory lib
put lib/libc.a "$LIBC_A"
put lib/libc-rust.o "$TCC_DIR/libc-rust.o"
tree "$TCC_DIR/lib/tcc" lib/tcc
