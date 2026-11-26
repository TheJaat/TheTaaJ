#!/bin/sh
# build-host-llvm.sh - the host's own LLVM 23.1.5: the clang, lld,
# llvm-tblgen, clang-tblgen and binutils that build TheTaaJ.
#
#   usage: build-host-llvm.sh [JOBS]
#
# Why the host needs its own (journal entry 104): TheTaaJ's LLVM port
# is 23.1.5, the release rustc 1.100 is built on. libc++ 23's headers
# need a clang of 21 or later, and the byte-for-byte checks (clang on
# TheTaaJ against the clang that built everything else) need the host's
# clang to be the same release as the port. Ubuntu 24.04 has clang 18.
#
# The source is llvm-project-23.1.5.src.tar.xz from the release on
# GitHub, its SHA-256 checked against the one below. Everything is in
# host-tools/ (git ignores it, and `make clean` leaves it alone): the
# tarball, the source, the build, and the installed tools in
# host-tools/llvm-23.1.5/bin, which the Makefile puts first on PATH.
#
# Needs cmake (3.20 or later), ninja, and a C++17 compiler (the host's
# clang 18). Takes one to two hours on 16 CPUs; run again, it builds
# only what changed, and does nothing if everything is installed.

set -eu
ROOT=$(cd "$(dirname "$0")/.." && pwd)
VERSION=23.1.5
SHA256=fb50b1eac93ddeb3aa8dab4cfe324dff74f4a9c2bdee6160c9e36188f8d06a6e
HOST=$ROOT/host-tools
TARBALL=$HOST/llvm-project-$VERSION.src.tar.xz
SOURCE=$HOST/llvm-project-$VERSION.src
BUILD=$HOST/build-llvm-$VERSION
PREFIX=$HOST/llvm-$VERSION
JOBS=${1:-$(nproc)}
# Linking LLVM's tools takes a few GB each: two at a time fit in 15 GB.
LINK_JOBS=${LINK_JOBS:-2}

TOOLS="clang lld llvm-tblgen clang-tblgen llc llvm-mc llvm-ar llvm-ranlib llvm-objcopy llvm-strip llvm-nm llvm-readelf llvm-objdump llvm-config"
done_already() {
    for tool in clang ld.lld llvm-tblgen clang-tblgen llc llvm-mc llvm-ar llvm-objcopy; do
        [ -x "$PREFIX/bin/$tool" ] || return 1
    done
    "$PREFIX/bin/clang" --version | grep -q "^clang version $VERSION\$"
}
if done_already; then
    exit 0
fi

command -v cmake > /dev/null || { echo "build-host-llvm: cmake not found (sudo apt install cmake)" >&2; exit 1; }
command -v ninja > /dev/null || { echo "build-host-llvm: ninja not found (sudo apt install ninja-build)" >&2; exit 1; }
mkdir -p "$HOST"
if [ ! -d "$SOURCE" ]; then
    if [ ! -f "$TARBALL" ]; then
        echo "Downloading LLVM $VERSION's source..."
        curl -sSL -o "$TARBALL.part" \
            "https://github.com/llvm/llvm-project/releases/download/llvmorg-$VERSION/llvm-project-$VERSION.src.tar.xz"
        mv "$TARBALL.part" "$TARBALL"
    fi
    echo "$SHA256  $TARBALL" | sha256sum -c - > /dev/null \
        || { echo "build-host-llvm: $TARBALL is not the release (SHA-256 differs)" >&2; exit 1; }
    echo "Unpacking..."
    tar xJf "$TARBALL" -C "$HOST"
fi

# Release, no assertions, the two targets TheTaaJ runs on; clang and
# lld. The host's own clang and lld build it. LLVM_APPEND_VC_REV off:
# host-tools/ is inside TheTaaJ's checkout, and CMake would otherwise
# take TheTaaJ's git URL and commit for LLVM's and put them in every
# --version ("clang version 23.1.5 (git@github.com:...)").
echo "Building LLVM $VERSION for the host in $BUILD ($JOBS jobs; this takes a while)..."
cmake -G Ninja -S "$SOURCE/llvm" -B "$BUILD" \
    -DCMAKE_BUILD_TYPE=Release \
    -DCMAKE_INSTALL_PREFIX="$PREFIX" \
    -DCMAKE_C_COMPILER="$(command -v clang-18 || command -v clang)" \
    -DCMAKE_CXX_COMPILER="$(command -v clang++-18 || command -v clang++)" \
    -DLLVM_USE_LINKER=lld \
    -DLLVM_ENABLE_PROJECTS="clang;lld" \
    -DLLVM_TARGETS_TO_BUILD="X86;AArch64" \
    -DLLVM_ENABLE_ASSERTIONS=OFF \
    -DLLVM_INCLUDE_TESTS=OFF -DLLVM_INCLUDE_EXAMPLES=OFF -DLLVM_INCLUDE_BENCHMARKS=OFF \
    -DCLANG_INCLUDE_TESTS=OFF \
    -DLLVM_ENABLE_ZSTD=OFF -DLLVM_ENABLE_ZLIB=OFF -DLLVM_ENABLE_LIBXML2=OFF -DLLVM_ENABLE_TERMINFO=OFF \
    -DLLVM_PARALLEL_LINK_JOBS="$LINK_JOBS" \
    -DLLVM_INSTALL_UTILS=ON \
    -DLLVM_APPEND_VC_REV=OFF \
    -DLLVM_BUILD_UTILS=ON \
    > "$HOST/cmake-$VERSION.log"
ninja -C "$BUILD" -j "$JOBS" $TOOLS
# Installed: the tools (llc and llvm-mc are what the byte-for-byte
# checks compare TheTaaJ's with), clang's own headers (stddef.h, the intrinsics),
# and the two tblgens, which LLVM installs only on request.
ninja -C "$BUILD" install-clang install-clang-resource-headers install-lld install-llc install-llvm-mc \
    install-llvm-ar install-llvm-ranlib install-llvm-objcopy install-llvm-strip \
    install-llvm-nm install-llvm-readelf install-llvm-objdump install-llvm-config
mkdir -p "$PREFIX/bin"
cp -f "$BUILD/bin/clang-tblgen" "$BUILD/bin/llvm-tblgen" "$PREFIX/bin/"
for alias in clang++ clang-cpp; do
    [ -e "$PREFIX/bin/$alias" ] || ln -sf clang "$PREFIX/bin/$alias"
done
done_already || { echo "build-host-llvm: built, but $PREFIX/bin is not complete" >&2; exit 1; }
echo "LLVM $VERSION for the host: $PREFIX/bin"
