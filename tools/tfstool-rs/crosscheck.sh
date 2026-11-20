#!/bin/sh
# crosscheck.sh - hold the Rust tfstool to the C one.
#
# Two implementations of one format are only worth having if they agree.
# This checks three things:
#
#   1. each tool reads what the other wrote
#   2. the same operations produce byte-identical images
#   3. their dumps of the same image are identical, line for line
#
# Run from anywhere; it builds nothing. `make tfscheck` builds both tools
# first and then runs this.

set -eu

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
C_TOOL=$ROOT/build/tools/tfstool
RS_TOOL=$ROOT/tools/tfstool-rs/target/release/tfstool

for tool in "$C_TOOL" "$RS_TOOL"; do
    [ -x "$tool" ] || { echo "missing $tool - run 'make tfscheck'"; exit 1; }
done

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

printf 'a file on TaajFS\n' > "$WORK/small"
head -c 20000 /dev/urandom > "$WORK/multi"     # five blocks, last one partial
: > "$WORK/empty"

C()  { "$C_TOOL"  "$@" >/dev/null; }
RS() { "$RS_TOOL" "$@" >/dev/null; }
same() { cmp -s "$1" "$2" || { echo "FAIL: $3"; exit 1; }; echo "ok    $3"; }

# 1. C writes, Rust reads.
C mkfs "$WORK/c.img" 1024
C put  "$WORK/c.img" small.txt "$WORK/small"
C put  "$WORK/c.img" multi.bin "$WORK/multi"
"$RS_TOOL" get "$WORK/c.img" small.txt > "$WORK/out1"
"$RS_TOOL" get "$WORK/c.img" multi.bin > "$WORK/out2"
same "$WORK/small" "$WORK/out1" "Rust reads a small file the C tool wrote"
same "$WORK/multi" "$WORK/out2" "Rust reads a multi-block file the C tool wrote"

# 2. Rust writes, C reads.
RS mkfs "$WORK/r.img" 1024
RS put  "$WORK/r.img" small.txt "$WORK/small"
RS put  "$WORK/r.img" multi.bin "$WORK/multi"
RS put  "$WORK/r.img" empty     "$WORK/empty"
"$C_TOOL" get "$WORK/r.img" small.txt > "$WORK/out3"
"$C_TOOL" get "$WORK/r.img" multi.bin > "$WORK/out4"
"$C_TOOL" get "$WORK/r.img" empty     > "$WORK/out5"
same "$WORK/small" "$WORK/out3" "C reads a small file the Rust tool wrote"
same "$WORK/multi" "$WORK/out4" "C reads a multi-block file the Rust tool wrote"
same "$WORK/empty" "$WORK/out5" "C reads an empty file the Rust tool wrote"

# 3. Mixed: each tool changes the other's image.
RS rm  "$WORK/c.img" small.txt
RS put "$WORK/c.img" again.txt "$WORK/small"
C  rm  "$WORK/r.img" multi.bin
"$C_TOOL"  get "$WORK/c.img" again.txt > "$WORK/out6"
"$C_TOOL"  get "$WORK/c.img" multi.bin > "$WORK/out7"
"$RS_TOOL" get "$WORK/r.img" small.txt > "$WORK/out8"
same "$WORK/small" "$WORK/out6" "C reads a file the Rust tool added to a C image"
same "$WORK/multi" "$WORK/out7" "a file survives the Rust tool changing its directory"
same "$WORK/small" "$WORK/out8" "a file survives the C tool removing its neighbour"
if "$RS_TOOL" get "$WORK/c.img" small.txt >/dev/null 2>&1; then
    echo "FAIL: a file removed by Rust is still readable"; exit 1
fi
echo "ok    a removed file is gone"

# 4. Same operations, same bytes. The C `put` is create-then-write, so
#    the Rust side does `touch` then `write` to follow the same steps.
C  mkfs  "$WORK/c2.img" 1024
C  put   "$WORK/c2.img" small.txt "$WORK/small"
C  put   "$WORK/c2.img" multi.bin "$WORK/multi"
C  rm    "$WORK/c2.img" small.txt
RS mkfs  "$WORK/r2.img" 1024
RS touch "$WORK/r2.img" small.txt
RS write "$WORK/r2.img" small.txt 0 "$WORK/small"
RS touch "$WORK/r2.img" multi.bin
RS write "$WORK/r2.img" multi.bin 0 "$WORK/multi"
RS rm    "$WORK/r2.img" small.txt
same "$WORK/c2.img" "$WORK/r2.img" "identical operations give byte-identical images"

# 5. Both dumps of every image agree.
for img in c r c2; do
    "$C_TOOL"  dump "$WORK/$img.img" > "$WORK/$img.cdump"  || true
    "$RS_TOOL" dump "$WORK/$img.img" > "$WORK/$img.rdump"  || true
    same "$WORK/$img.cdump" "$WORK/$img.rdump" "both dumps of $img.img agree"
done

# 6. Directories are the Rust implementation's alone. A volume keeps
#    working with the C tool until the first directory is made on it;
#    from then on the C tool must refuse it - at every step, including
#    the ones in between - because it would take a directory for a file.
RS mkfs  "$WORK/d.img" 1024
RS put   "$WORK/d.img" small.txt "$WORK/small"
C  put   "$WORK/d.img" multi.bin "$WORK/multi"
RS mkdir "$WORK/d.img" docs
RS put   "$WORK/d.img" docs/small.txt "$WORK/small"
cp "$WORK/d.img" "$WORK/d.before"
refused() { echo "FAIL: the C tool did '$1' on a volume with directories"; exit 1; }
! "$C_TOOL" ls  "$WORK/d.img"                         >/dev/null 2>&1 || refused ls
! "$C_TOOL" get "$WORK/d.img" small.txt               >/dev/null 2>&1 || refused get
! "$C_TOOL" put "$WORK/d.img" again.txt "$WORK/small" >/dev/null 2>&1 || refused put
! "$C_TOOL" rm  "$WORK/d.img" small.txt               >/dev/null 2>&1 || refused rm
same "$WORK/d.before" "$WORK/d.img" "the C tool's attempts changed nothing on it"
echo "ok    the C tool refuses a volume that has had a directory made on it"
"$RS_TOOL" get "$WORK/d.img" docs/small.txt > "$WORK/out9"
"$RS_TOOL" get "$WORK/d.img" multi.bin      > "$WORK/out10"
same "$WORK/small" "$WORK/out9"  "a file in a directory reads back"
same "$WORK/multi" "$WORK/out10" "and the file the C tool wrote before is intact"
"$RS_TOOL" dump "$WORK/d.img" > /dev/null || { echo "FAIL: the volume with directories does not dump clean"; exit 1; }
echo "ok    the volume with directories dumps clean"

# 7. A rename is new to the Rust implementation, but not to the format:
#    on a volume with no directories it writes only what the C tool
#    already understands - a directory block with another name in it.
#    So the C tool must read the result, and both dumps must agree.
RS mkfs "$WORK/m.img" 1024
C  put  "$WORK/m.img" first.txt "$WORK/small"
C  put  "$WORK/m.img" multi.bin "$WORK/multi"
RS mv   "$WORK/m.img" multi.bin moved.bin
"$C_TOOL" get "$WORK/m.img" moved.bin > "$WORK/out11"
same "$WORK/multi" "$WORK/out11" "the C tool reads a file the Rust tool renamed"
! "$C_TOOL" get "$WORK/m.img" multi.bin >/dev/null 2>&1 || { echo "FAIL: the old name is still there for the C tool"; exit 1; }
"$C_TOOL"  dump "$WORK/m.img" > "$WORK/m.cdump" || true
"$RS_TOOL" dump "$WORK/m.img" > "$WORK/m.rdump" || true
same "$WORK/m.cdump" "$WORK/m.rdump" "both dumps of the renamed volume agree"

echo "crosscheck passed"
