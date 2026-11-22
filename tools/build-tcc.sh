#!/bin/sh
# build-tcc.sh - TCC for TheTaaJ, for one CPU: a cross compiler that
# runs here, and the compiler that runs on TheTaaJ.
#
#   usage: build-tcc.sh ARCH OUT LIBC_A CFLAGS LDFLAGS
#
#   ARCH     x86_64 or aarch64
#   OUT      the directory to build in (made if need be)
#   LIBC_A   TheTaaJ's C library for that CPU, already built
#   CFLAGS   clang's flags for a TheTaaJ C program (TAAJ_CFLAGS)
#   LDFLAGS  ld.lld's (TAAJ_LDFLAGS)
#
# What it leaves in OUT:
#
#   cross/taaj-tcc   TCC for this machine, writing TheTaaJ programs; it
#                    finds the headers in libc/include and libc.a beside
#                    LIBC_A, as tcc on TheTaaJ finds them in /include
#                    and /lib
#   lib/tcc/         what goes in /lib/tcc on a TheTaaJ disk: TCC's own
#                    headers (include/: stddef.h, stdarg.h, tccdefs.h,
#                    ...) and libtcc1.a, its helpers
#   tcc              TCC itself, compiled by clang, linked with LIBC_A:
#                    the program that runs on TheTaaJ
#
# libtcc1.a is compiled by the cross TCC - the helpers are what code TCC
# writes calls, so TCC compiles them, as upstream's own build does. On
# AArch64 it leaves out lib-arm64.c (the 128-bit long double arithmetic):
# libc.a has those already (libc/c/fp128.c, a copy of it), and two of
# each in one link is a mistake waiting to happen.
#
# The upstream sources are compiled as they come, without -Wall and
# -Werror: they are not TheTaaJ's code to keep warning-free.

set -eu
ARCH=$1
OUT=$2
LIBC_A=$3
CFLAGS=$4
LDFLAGS=$5
SRC=$(cd "$(dirname "$0")/../ports/tcc" && pwd)
ROOT=$(cd "$(dirname "$0")/.." && pwd)
VERSION=$(cat "$SRC/VERSION")

case $ARCH in
    x86_64)  TARGET=TCC_TARGET_X86_64; HELPERS="libtcc1.c alloca.S builtin.c stdatomic.c atomic.S" ;;
    aarch64) TARGET=TCC_TARGET_ARM64;  HELPERS="alloca.S builtin.c stdatomic.c atomic.S" ;;
    *) echo "build-tcc: no such CPU: $ARCH" >&2; exit 2 ;;
esac

mkdir -p "$OUT/cross" "$OUT/native" "$OUT/helpers" "$OUT/lib/tcc"
OUT=$(cd "$OUT" && pwd)
LIBC_DIR=$(cd "$(dirname "$LIBC_A")" && pwd)

# The configuration upstream's configure script would write. Both say
# TARGETOS_TheTaaJ (the paths, no crt files, static only, __TAAJ__ and
# not __linux__: ports/tcc/tcc.h, tccpp.c); the cross compiler's paths
# are this machine's, and it keeps the host's own threads and libdl.
cat > "$OUT/cross/config.h" <<EOF
#define TCC_VERSION "$VERSION"
#define TARGETOS_TheTaaJ 1
#define CONFIG_TCCDIR "$OUT/lib/tcc"
#define CONFIG_TCC_SYSINCLUDEPATHS "{B}/include:$ROOT/libc/include"
#define CONFIG_TCC_LIBPATHS "{B}:$LIBC_DIR"
#define CONFIG_TCC_BACKTRACE 0
#define CONFIG_TCC_BCHECK 0
EOF
# On TheTaaJ: no dlopen (CONFIG_TCC_STATIC), no threads to lock against
# (CONFIG_TCC_SEMLOCK 0), and no backtraces or bounds checking, which
# need both.
cat > "$OUT/native/config.h" <<EOF
#define TCC_VERSION "$VERSION"
#define TARGETOS_TheTaaJ 1
#define CONFIG_TCCDIR "/lib/tcc"
#define CONFIG_TCC_STATIC 1
#define CONFIG_TCC_SEMLOCK 0
#define CONFIG_TCC_BACKTRACE 0
#define CONFIG_TCC_BCHECK 0
EOF

echo "  CC      ports/tcc/tcc.c (cross, $ARCH)"
${HOST_CC:-cc} -O2 -w -D$TARGET -I"$OUT/cross" "$SRC/tcc.c" -o "$OUT/cross/taaj-tcc" -lm -lpthread -ldl

rm -rf "$OUT/lib/tcc/include" "$OUT/helpers"/*.o
cp -r "$SRC/include" "$OUT/lib/tcc/include"
for helper in $HELPERS; do
    echo "  TCC     ports/tcc/lib/$helper ($ARCH)"
    "$OUT/cross/taaj-tcc" -c "$SRC/lib/$helper" -o "$OUT/helpers/${helper%.*}.o"
done
rm -f "$OUT/lib/tcc/libtcc1.a"
"$OUT/cross/taaj-tcc" -ar rcs "$OUT/lib/tcc/libtcc1.a" "$OUT"/helpers/*.o

echo "  CC      ports/tcc/tcc.c ($ARCH)"
# shellcheck disable=SC2086 # the flags are words
clang $(echo " $CFLAGS " | sed -e 's/ -Wall / /' -e 's/ -Wextra / /' -e 's/ -Werror / /' -e 's/ -std=c11 / -std=gnu11 /') \
    -D$TARGET -I"$OUT/native" -c "$SRC/tcc.c" -o "$OUT/native/tcc.o"
# shellcheck disable=SC2086
ld.lld $LDFLAGS "$OUT/native/tcc.o" "$LIBC_A" -o "$OUT/tcc"
