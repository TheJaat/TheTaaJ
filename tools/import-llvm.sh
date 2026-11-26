#!/bin/sh
# import-llvm.sh - puts a release of LLVM into ports/llvm, as it is
# upstream: the parts TheTaaJ builds, the way ports/llvm/UPSTREAM lists
# them. TheTaaJ's own changes are made after, in commits of their own,
# so the history shows upstream and TheTaaJ apart.
#
#   usage: import-llvm.sh SOURCE
#
# SOURCE is an unpacked llvm-project-X.Y.Z.src (tools/build-host-llvm.sh
# unpacks 23.1.5 into host-tools/). What is in ports/llvm now for those
# parts is replaced; ports/llvm/taaj (TheTaaJ's configuration) and
# UPSTREAM stay.

set -eu
SOURCE=$1
ROOT=$(cd "$(dirname "$0")/.." && pwd)
PORT=$ROOT/ports/llvm
[ -f "$SOURCE/llvm/CMakeLists.txt" ] || { echo "import-llvm: $SOURCE is not an llvm-project tree" >&2; exit 1; }

# copy PART...: each path, from SOURCE to the port, replacing it.
copy() {
    for part in "$@"; do
        rm -rf "${PORT:?}/$part"
        mkdir -p "$(dirname "$PORT/$part")"
        cp -R "$SOURCE/$part" "$PORT/$part"
    done
}

# The C++ runtimes: their headers and sources (with their CMakeLists.txt,
# the lists of what they are made of) and licences.
copy LICENSE.TXT
copy libcxx/include libcxx/src libcxx/vendor libcxx/LICENSE.TXT
copy libcxxabi/include libcxxabi/src libcxxabi/LICENSE.TXT
copy libunwind/include libunwind/src libunwind/LICENSE.TXT

# LLVM's libc, the part LLVM and libc++ build on since 20: its "shared"
# headers - floating-point numbers read and written exactly (libc++'s
# from_chars), math correctly rounded (LLVM's APFloat) - and the support
# code and headers they include. Not the C library itself; and not the
# 12 MB table of powers of ten for printing long doubles, which only
# printf uses.
copy libc/shared libc/hdr libc/include/llvm-libc-macros libc/include/llvm-libc-types libc/src/__support libc/LICENSE.TXT
rm -f "$PORT/libc/src/__support/ryu_long_double_constants.h"

# SipHash, the hash LLVM's Support uses for pointer authentication,
# from third-party/ (one header).
copy third-party/siphash

# LLVM: every library, but of the targets only X86 and AArch64 (and the
# files common to all), and the .td files of ARM, PowerPC and RISC-V -
# the TargetParser is written from ARM.td, PPC.td and RISCV.td whatever
# the targets.
copy llvm/include llvm/lib llvm/tools/llc llvm/tools/llvm-mc llvm/utils/TableGen llvm/LICENSE.TXT
for target in "$PORT"/llvm/lib/Target/*/; do
    case $(basename "$target") in
    X86 | AArch64) ;;
    ARM | PowerPC | RISCV) find "$target" -type f ! -name '*.td' -delete ;;
    *) rm -rf "$target" ;;
    esac
done
find "$PORT/llvm/lib/Target/ARM" "$PORT/llvm/lib/Target/PowerPC" "$PORT/llvm/lib/Target/RISCV" -type d -empty -delete

# Clang: the libraries the clang program is made of - not the static
# analyser, the interpreter, CIR, the directory watcher, the other
# tools' libraries; of Tooling only Core and Inclusions. (Since 23 the
# driver needs the dependency scanner, and the front end the scalable
# static analysis and what it is built on: they stay.)
copy clang/CMakeLists.txt clang/include clang/lib clang/tools/driver clang/utils/TableGen clang/LICENSE.TXT
for library in CIR CrossTU DirectoryWatcher IndexSerialization Interpreter StaticAnalyzer Testing; do
    rm -rf "$PORT/clang/lib/$library"
done
for tool in "$PORT"/clang/lib/Tooling/*; do
    case $(basename "$tool") in
    Core | Inclusions) ;;
    *) rm -rf "$tool" ;;
    esac
done

# lld: the ELF linker only.
copy lld/CMakeLists.txt lld/include lld/Common lld/ELF lld/tools/lld lld/LICENSE.TXT

echo "import-llvm: $(basename "$SOURCE") in ports/llvm ($(du -sh "$PORT" | cut -f1))"
