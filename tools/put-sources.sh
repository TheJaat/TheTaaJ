#!/bin/sh
# put-sources.sh - puts sources on a TaajFS disk, with the scripts that
# build them there:
#
#   /src/build             libc/build.taaj: `source /src/build`
#   /src/libc/c/...        the C half of the C library (libm/: musl's
#                          sin, exp, pow and the rest)
#   /src/libc/programs/... the C programs
#   /src/tcc/build         ports/tcc-ARCH.taaj: `source /src/tcc/build`
#   /src/tcc/...           TCC's sources, the ones for ARCH
#   /src/tcc/config.h      the configuration /tcc was built with, and ARCH
#   /src/tcc/lib/...       the sources of TCC's library, libtcc1.a
#
#   usage: put-sources.sh DISK ARCH TCC_DIR
#
# ARCH is x86_64 or aarch64; TCC_DIR is what tools/build-tcc.sh built
# for it. The headers are in /include already (tools/put-tcc.sh), and
# the C library's Rust half in /lib/libc-rust.o. A file already on the
# disk is replaced.

set -eu
DISK=$1
ARCH=$2
TCC_DIR=$3
ROOT=$(cd "$(dirname "$0")/.." && pwd)
TFSTOOL=${TFSTOOL:-$ROOT/tools/tfstool-rs/target/release/tfstool}

put() {
    "$TFSTOOL" rm "$DISK" "$1" > /dev/null 2>&1 || true
    "$TFSTOOL" put "$DISK" "$1" "$2" > /dev/null || { echo "put-sources: cannot put $1 on $DISK" >&2; exit 1; }
}
directory() {
    "$TFSTOOL" mkdir "$DISK" "$1" > /dev/null 2>&1 || true
}

directory src
directory src/libc
directory src/libc/c
directory src/libc/programs
put src/build "$ROOT/libc/build.taaj"
for f in "$ROOT"/libc/c/*.c "$ROOT"/libc/c/*.h; do
    case $(basename "$f") in
        *_test.c) ;; # the host's tests of the library, not part of it
        *) put "src/libc/c/$(basename "$f")" "$f" ;;
    esac
done
directory src/libc/c/libm
for f in "$ROOT"/libc/c/libm/*.c "$ROOT"/libc/c/libm/*.h; do put "src/libc/c/libm/$(basename "$f")" "$f"; done
for f in "$ROOT"/libc/programs/*.c; do put "src/libc/programs/$(basename "$f")" "$f"; done

# TCC's sources: tcc.c includes the rest (ONE_SOURCE), and of the
# CPU-specific files only those for ARCH.
SRC=$ROOT/ports/tcc
COMMON="tcc.c libtcc.c tcc.h libtcc.h tccpp.c tccgen.c tccdbg.c tccasm.c tccelf.c tccrun.c tcctools.c tcctok.h elf.h stab.h stab.def dwarf.h"
case $ARCH in
    x86_64)  TARGET=TCC_TARGET_X86_64; FILES="x86_64-gen.c x86_64-link.c i386-asm.c i386-asm.h x86_64-asm.h i386-tok.h"
             HELPERS="libtcc1.c va_list.c alloca.S builtin.c stdatomic.c atomic.S" ;;
    aarch64) TARGET=TCC_TARGET_ARM64;  FILES="arm64-gen.c arm64-link.c arm64-asm.c arm64-tok.h"
             HELPERS="alloca.S builtin.c stdatomic.c atomic.S" ;;
    *) echo "put-sources: no such CPU: $ARCH" >&2; exit 2 ;;
esac
directory src/tcc
directory src/tcc/lib
put src/tcc/build "$ROOT/ports/tcc-$ARCH.taaj"
for f in $COMMON $FILES; do put "src/tcc/$f" "$SRC/$f"; done
for f in $HELPERS; do put "src/tcc/lib/$f" "$SRC/lib/$f"; done
CONFIG=$(mktemp)
{ cat "$TCC_DIR/native/config.h"; echo "#define $TARGET 1"; } > "$CONFIG"
put src/tcc/config.h "$CONFIG"
rm -f "$CONFIG"
