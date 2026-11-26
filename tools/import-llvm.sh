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

# LLVM: every library, but of the targets only X86 and AArch64 (and the
# files common to all), and RISC-V's .td files - the TargetParser is
# written from RISCV.td whatever the targets.
copy llvm/include llvm/lib llvm/tools/llc llvm/tools/llvm-mc llvm/utils/TableGen llvm/LICENSE.TXT
for target in "$PORT"/llvm/lib/Target/*/; do
    case $(basename "$target") in
    X86 | AArch64) ;;
    RISCV) find "$target" -type f ! -name '*.td' -delete ;;
    *) rm -rf "$target" ;;
    esac
done
find "$PORT/llvm/lib/Target/RISCV" -type d -empty -delete

# Clang: the libraries the clang program is made of - not the static
# analyser, the interpreter, CIR, the dependency scanner, the install
# API, the other tools' libraries; of Tooling only Core and Inclusions.
copy clang/CMakeLists.txt clang/include clang/lib clang/tools/driver clang/utils/TableGen clang/LICENSE.TXT
for library in CIR CrossTU DependencyScanning DirectoryWatcher IndexSerialization InstallAPI Interpreter \
    ScalableStaticAnalysis StaticAnalyzer Testing UnifiedSymbolResolution; do
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
