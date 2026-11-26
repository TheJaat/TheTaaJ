#!/bin/sh
# build-cxx.sh - C++ for TheTaaJ, for one CPU: LLVM's C++ runtimes
# (ports/llvm), cross-compiled by clang.
#
#   usage: build-cxx.sh ARCH OUT CXXFLAGS
#
#   ARCH      x86_64 or aarch64
#   OUT       the directory to build in (made if need be)
#   CXXFLAGS  clang's flags for a TheTaaJ C++ program (TAAJ_CXXFLAGS),
#             without -Wall, -Wextra and -Werror - they are added for
#             TheTaaJ's own programs, not for upstream's code - and
#             without the include paths, which this script adds
#
# What it leaves in OUT:
#
#   include/c++/v1/  libc++'s headers, its __config_site
#                    (ports/llvm/taaj), and libc++abi's cxxabi.h
#   include/         libunwind's unwind.h and libunwind.h
#   libc++.a         the three libraries in one archive, so a program
#                    is linked with libc++.a and libc.a and nothing else:
#
#     libunwind   walks the stack when an exception is thrown, reading
#                 the tables (.eh_frame) the compiler writes for each
#                 function; on TheTaaJ it finds them between symbols
#                 that user/cxx.ld defines (_LIBUNWIND_IS_BAREMETAL)
#     libc++abi   the language's runtime: throw and catch, typeinfo
#                 and dynamic_cast, new and delete, the guards of
#                 function-local statics, the demangler
#     libc++      the standard library: strings, containers,
#                 algorithms, iostreams, ...
#
# The parts of libc++ that need what TheTaaJ does not have yet -
# directories, entropy, wide characters, time zones - are left out (see
# ports/llvm/taaj/__config_site). Upstream's own build is CMake; the
# lists of sources here follow its CMakeLists.txt for that configuration.
# Threads are in, on the C library's pthreads (libc/include/pthread.h,
# libc/src/threads.rs); thread_local variables are ELF thread-local
# storage, as in C and Rust (entry 104: no more -femulated-tls).
#
# KEEP_GOING=1 in the environment goes on past a file that does not
# compile, to see every error at once - for porting; the archive made
# then is missing those files.

set -eu
ARCH=$1
OUT=$2
CXXFLAGS=$3
ROOT=$(cd "$(dirname "$0")/.." && pwd)
LLVM=$ROOT/ports/llvm

case $ARCH in
    x86_64|aarch64) ;;
    *) echo "build-cxx: no such CPU: $ARCH" >&2; exit 2 ;;
esac

mkdir -p "$OUT"
OUT=$(cd "$OUT" && pwd)
rm -rf "$OUT/include" "$OUT/obj"
mkdir -p "$OUT/include/c++/v1" "$OUT/obj/unwind" "$OUT/obj/cxxabi" "$OUT/obj/cxx"

# The headers, as a program sees them.
cp -r "$LLVM/libcxx/include/." "$OUT/include/c++/v1/"
rm -f "$OUT/include/c++/v1/CMakeLists.txt" "$OUT/include/c++/v1/__config_site.in"
cp "$LLVM/taaj/__config_site" "$OUT/include/c++/v1/"
# What a failed check inside libc++ does (none are on: hardening is
# off in __config_site): upstream's default, as CMake would put it.
cp "$LLVM/libcxx/vendor/llvm/default_assertion_handler.in" "$OUT/include/c++/v1/__assertion_handler"
cp "$LLVM/libcxxabi/include/cxxabi.h" "$LLVM/libcxxabi/include/__cxxabi_config.h" "$OUT/include/c++/v1/"
for f in unwind.h unwind_arm_ehabi.h unwind_itanium.h libunwind.h __libunwind_config.h; do
    cp "$LLVM/libunwind/include/$f" "$OUT/include/"
done

INCLUDES="-nostdinc++ -isystem $OUT/include/c++/v1 -isystem $OUT/include -isystem $ROOT/libc/include"
# shellcheck disable=SC2086 # the flags are words
compile() { # compile OUTDIR SOURCE EXTRA-FLAGS...
    dir=$1; src=$2; shift 2
    obj=$dir/$(basename "$src" | sed 's/\.[^.]*$//').o
    echo "  CXX     ports/llvm/${src#"$LLVM"/} ($ARCH)"
    case $src in
        *.c) clang $(echo " $CXXFLAGS " | sed -e 's/ -std=[^ ]* / -std=c11 /') $INCLUDES "$@" -c "$src" -o "$obj" ;;
        *)   clang $(echo " $CXXFLAGS " | sed -e "s/ -std=[^ ]* / -std=$STD /") $INCLUDES "$@" -c "$src" -o "$obj" ;;
    esac || [ -n "${KEEP_GOING:-}" ]
}

# Each library in the C++ its upstream build uses: libunwind 17,
# libc++abi 23, libc++ 26 (programs get CXXFLAGS' own).
#
# libunwind: C, C++ without exceptions of its own, and the assembly
# that saves and restores every register.
STD=c++17
for src in libunwind.cpp UnwindLevel1.c UnwindLevel1-gcc-ext.c UnwindRegistersRestore.S UnwindRegistersSave.S; do
    compile "$OUT/obj/unwind" "$LLVM/libunwind/src/$src" \
        -I"$LLVM/libunwind/include" -D_LIBUNWIND_IS_BAREMETAL -D_LIBUNWIND_HAS_NO_THREADS \
        -fno-exceptions -fno-rtti -funwind-tables
done

# libc++abi.
STD=c++23
for src in cxa_aux_runtime.cpp cxa_default_handlers.cpp cxa_demangle.cpp cxa_exception_storage.cpp \
           cxa_guard.cpp cxa_handlers.cpp cxa_vector.cpp cxa_virtual.cpp stdlib_exception.cpp \
           stdlib_stdexcept.cpp stdlib_typeinfo.cpp abort_message.cpp fallback_malloc.cpp \
           private_typeinfo.cpp stdlib_new_delete.cpp cxa_exception.cpp cxa_personality.cpp \
           cxa_thread_atexit.cpp; do
    compile "$OUT/obj/cxxabi" "$LLVM/libcxxabi/src/$src" \
        -I"$LLVM/libcxxabi/include" -I"$LLVM/libcxx/src" \
        -D_LIBCXXABI_BUILDING_LIBRARY -D_LIBCPP_BUILDING_LIBRARY \
        -DLIBCXXABI_USE_LLVM_UNWINDER
done

# libc++. Its from_chars for floating-point numbers is LLVM libc's
# (ports/llvm/libc/shared), in a namespace of its own, as upstream's
# llvm-libc-common-utilities has it.
STD=c++26
# (Its sources wrap themselves in `#pragma clang attribute push` regions
# of ABI attributes - visibility, abi_tag - and clang 23 says, for each
# such region, which attributes found nothing there to apply to:
# hundreds of lines about upstream's code, not TheTaaJ's, hence the two
# -Wno- below.)
# (filesystem/path.cpp and its two companions are in every build, as in
# upstream's list: without _LIBCPP_HAS_FILESYSTEM they come out empty,
# like fstream.cpp.)
for src in algorithm.cpp any.cpp bind.cpp call_once.cpp charconv.cpp chrono.cpp error_category.cpp \
           exception.cpp expected.cpp filesystem/filesystem_clock.cpp filesystem/filesystem_error.cpp \
           filesystem/path.cpp functional.cpp hash.cpp memory.cpp memory_resource.cpp \
           new_handler.cpp new_helpers.cpp optional.cpp print.cpp random_shuffle.cpp ryu/d2fixed.cpp \
           ryu/d2s.cpp ryu/f2s.cpp stdexcept.cpp string.cpp system_error.cpp typeinfo.cpp valarray.cpp \
           variant.cpp vector.cpp verbose_abort.cpp \
           fstream.cpp ios.cpp ios.instantiations.cpp iostream.cpp locale.cpp ostream.cpp regex.cpp \
           strstream.cpp text_encoding.cpp \
           atomic.cpp barrier.cpp condition_variable_destructor.cpp condition_variable.cpp future.cpp \
           mutex_destructor.cpp mutex.cpp shared_mutex.cpp thread.cpp; do
    compile "$OUT/obj/cxx" "$LLVM/libcxx/src/$src" \
        -I"$LLVM/libcxx/src" -I"$LLVM/libcxxabi/include" -isystem "$LLVM/libc" \
        -DLIBC_NAMESPACE=__llvm_libc_common_utils \
        -D_LIBCPP_BUILDING_LIBRARY -DLIBCXX_BUILDING_LIBCXXABI \
        -Wno-user-defined-literals -Wno-pragma-clang-attribute -Wno-ignored-attributes
done

echo "  AR      libc++.a ($ARCH)"
rm -f "$OUT/libc++.a"
"${LLVM_AR:-llvm-ar}" rcs "$OUT/libc++.a" "$OUT"/obj/unwind/*.o "$OUT"/obj/cxxabi/*.o "$OUT"/obj/cxx/*.o
