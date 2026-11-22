#!/bin/sh
# build-clang-tblgen.sh - clang-tblgen, for the host: the program that
# writes Clang's tables (its diagnostics, attributes, options, the
# builtins, arm_neon.h ...) from the .td files, as llvm-tblgen does
# LLVM's. Clang's build runs it on the machine that builds; it is not
# part of what runs on TheTaaJ.
#
#   usage: build-clang-tblgen.sh OUT
#
# Built from ports/llvm/clang/utils/TableGen - the same release as the
# Clang it writes for, which is what makes its output right - against
# the host's LLVM of that release (llvm-config-18: Ubuntu's llvm-18-dev
# package). The host's shared libLLVM lacks LLVM's TableGen library, so
# that one is linked from its archive. Built again only when a source
# is newer than the program.

set -eu
OUT=$1
ROOT=$(cd "$(dirname "$0")/.." && pwd)
CLANG=$ROOT/ports/llvm/clang
CONFIG=${LLVM_CONFIG:-$(command -v llvm-config-18 || command -v llvm-config || true)}
CXX=${HOST_CXX:-$(command -v clang++-18 || command -v clang++)}
[ -n "$CONFIG" ] || { echo "build-clang-tblgen: llvm-config not found (sudo apt install llvm-18-dev)" >&2; exit 1; }
"$CONFIG" --version | grep -q '^18\.1\.3' \
    || { echo "build-clang-tblgen: the host's LLVM is $("$CONFIG" --version), not 18.1.3" >&2; exit 1; }
LIBDIR=$("$CONFIG" --libdir)
[ -f "$LIBDIR/libLLVMTableGen.a" ] || { echo "build-clang-tblgen: no $LIBDIR/libLLVMTableGen.a (sudo apt install llvm-18-dev)" >&2; exit 1; }

# The sources: clang-tblgen's own, and the one file of clangSupport it
# shares with Clang (utils/TableGen/CMakeLists.txt: clangSupport_tablegen).
SOURCES="$(ls "$CLANG"/utils/TableGen/*.cpp) $CLANG/lib/Support/RISCVVIntrinsicUtils.cpp"
PROGRAM=$OUT/clang-tblgen
if [ -x "$PROGRAM" ] && [ -z "$(find $SOURCES "$CLANG/include" "$0" -newer "$PROGRAM" | head -1)" ]; then
    exit 0
fi
echo "  CXX     clang-tblgen (for the host)"
mkdir -p "$OUT"
for source in $SOURCES; do
    echo "$source"
done | xargs -P "$(nproc)" -I{} sh -c \
    '"$1" $("$2" --cxxflags) -I "$3/include" -O2 -w -c "$4" -o "$5/$(basename "$4" .cpp).o"' \
    sh "$CXX" "$CONFIG" "$CLANG" {} "$OUT"
OBJECTS=""
for source in $SOURCES; do
    OBJECTS="$OBJECTS $OUT/$(basename "$source" .cpp).o"
done
"$CXX" $OBJECTS $("$CONFIG" --ldflags) "$LIBDIR/libLLVMTableGen.a" -lLLVM-18 -o "$PROGRAM.new"
mv -f "$PROGRAM.new" "$PROGRAM"
