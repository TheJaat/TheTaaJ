#!/bin/sh
# put-sources.sh - puts the C library's and the C programs' sources on
# a TaajFS disk, with the script that builds them there:
#
#   /src/build             libc/build.taaj: `source /src/build`
#   /src/libc/c/...        the C half of the C library
#   /src/libc/programs/... the C programs
#
#   usage: put-sources.sh DISK
#
# The headers are in /include already (tools/put-tcc.sh), and the Rust
# half in /lib/libc-rust.o. A file already on the disk is replaced.

set -eu
DISK=$1
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
for f in "$ROOT"/libc/programs/*.c; do put "src/libc/programs/$(basename "$f")" "$f"; done
