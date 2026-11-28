#!/bin/sh
# fuzz.sh - the parsers of untrusted input fuzzed on the host (K189,
# journal entry NNN): fuzz/src/bin/*.rs, one target each, built with
# LLVM's coverage hooks and LLVM's libFuzzer - no crate from outside.
#
#   tools/fuzz.sh build             builds every target
#   tools/fuzz.sh run [SECONDS] [TARGET...]
#                                   each target (default: all) for
#                                   SECONDS (default 60), one after
#                                   another, FUZZ_JOBS processes each
#   tools/fuzz.sh list              the targets
#
# What each target found to be interesting is kept between runs in
# build/fuzz/corpus/TARGET, so a run goes on where the last one stopped.
# An input that crashes a target (a panic: an index out of range, an
# overflow, an unwrap) is written to build/fuzz/crashes/TARGET/ and the
# run says so and fails. A crash found is fixed in the parser, with a
# host test of that input beside the parser's other tests (K104's way).
#
# libFuzzer: libclang_rt.fuzzer-x86_64.a, from FUZZER_LIB, else from the
# project's LLVM 18.1.3 (~/opt/llvm-18.1.3), else from the host's clang
# (`clang -print-file-name`; Ubuntu: apt install libclang-rt-dev).
set -u
ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT/fuzz" || exit 1
TRIPLE=x86_64-unknown-linux-gnu
OUT=$ROOT/build/fuzz
BIN=$ROOT/fuzz/target/$TRIPLE/release

targets() { ls src/bin | sed -n 's/\.rs$//p'; }

library() {
    for lib in ${FUZZER_LIB:-} "$HOME"/opt/llvm-18.1.3/usr/lib/llvm-*/lib/clang/*/lib/linux/libclang_rt.fuzzer-x86_64.a \
               "$(clang -print-file-name=libclang_rt.fuzzer-x86_64.a 2>/dev/null)"; do
        [ -f "$lib" ] && { echo "$lib"; return 0; }
    done
    echo "fuzz: no libFuzzer (libclang_rt.fuzzer-x86_64.a): set FUZZER_LIB, or apt install libclang-rt-dev" >&2
    return 1
}

build() {
    lib=$(library) || exit 1
    # sancov: a counter on every branch, the table of where they are, and
    # the comparisons' operands - what libFuzzer steers by (the flags
    # `cargo fuzz` gives). With --target, they reach the targets and the
    # parsers' crates, not build scripts.
    RUSTFLAGS="-C passes=sancov-module -C llvm-args=-sanitizer-coverage-level=4 \
-C llvm-args=-sanitizer-coverage-inline-8bit-counters -C llvm-args=-sanitizer-coverage-pc-table \
-C llvm-args=-sanitizer-coverage-trace-compares -C link-arg=$lib -C link-arg=-lstdc++ --cfg fuzzing" \
        cargo build --release --quiet --target $TRIPLE --bins
}

# A target's first inputs, when its corpus is empty: what the build
# already has - real programs, QEMU's own device tree, the TCP test's
# frames - so the fuzzer starts from valid input, not from nothing (an
# ELF file is refused below 64 bytes, and libFuzzer's first inputs are
# a few bytes long).
seed() { # seed TARGET DIR
    case $1 in
    elf)
        for f in "$ROOT"/user/target/*-none*/release/* "$ROOT"/build/c/*; do
            [ -f "$f" ] && [ ! -x "$f.d" ] && [ "$(head -c 4 "$f" | od -An -tx1 | tr -d ' ')" = 7f454c46 ] &&
                [ "$(stat -c %s "$f")" -lt 65536 ] && cp "$f" "$2/$(basename "$(dirname "$(dirname "$f")")")-$(basename "$f")"
        done 2>/dev/null
        ;;
    devicetree)
        qemu-system-aarch64 -machine virt,dumpdtb="$2/virt.dtb" -cpu cortex-a57 -nographic > /dev/null 2>&1
        qemu-system-aarch64 -machine virt,dumpdtb="$2/virt-smp4.dtb" -cpu cortex-a57 -smp 4 -m 1G -nographic > /dev/null 2>&1
        ;;
    net_wire)
        # Frames from the TCP test's packet file (tcp-test.sh), if built.
        python3 -I - "$ROOT/build/tcp64.log.pcap" "$2" <<'PCAP' 2>/dev/null
import struct, sys
data = open(sys.argv[1], "rb").read()
at, n = 24, 0
while at + 16 <= len(data) and n < 40:
    size = struct.unpack("<I", data[at + 8:at + 12])[0]
    if n % 25 == 0 or size < 80:
        open(f"{sys.argv[2]}/frame-{n}", "wb").write(data[at + 16:at + 16 + size])
    at += 16 + size
    n += 1
PCAP
        ;;
    acpi_aml)
        # QEMU's DSDT as acpi/src/tests.rs makes it: a scope, a device,
        # Name (\_S5, Package (4) { Zero, Zero, Zero, Zero }), a method.
        python3 -I -c '
import struct, sys
body = bytes([0x10, 0x0B]) + b"\\_SB_" + bytes([0x5B, 0x82, 0x05]) + b"PCI0" + bytes([0x0D]) + b"_S5_?" + bytes([0])
body += bytes([0x08]) + b"\\_S5_" + bytes([0x12, 0x06, 0x04, 0, 0, 0, 0])
body += bytes([0x14, 0x08]) + b"_PTS" + bytes([0x01, 0xA3])
table = b"DSDT" + struct.pack("<I", 36 + len(body)) + bytes(28) + body
open(sys.argv[1], "wb").write(table)' "$2/qemu-dsdt"
        ;;
    http)
        printf 'GET /index.html HTTP/1.1\r\nHost: 10.0.2.15\r\nContent-Length: 0\r\n\r\n' > "$2/get"
        ;;
    esac
}

case ${1:-} in
build)
    build
    ;;
list)
    targets
    ;;
run)
    seconds=${2:-60}
    shift $(( $# < 2 ? $# : 2 ))
    list=${*:-$(targets)}
    build || exit 1
    # FUZZ_JOBS processes (default a quarter of the CPUs: by day, other
    # sessions' timed boots share the machine; at night, more).
    jobs=${FUZZ_JOBS:-$(( $(nproc) > 4 ? $(nproc) / 4 : 1 ))}
    failed=
    for target in $list; do
        corpus=$OUT/corpus/$target
        crashes=$OUT/crashes/$target
        mkdir -p "$corpus" "$crashes"
        [ -n "$(ls "$corpus")" ] || seed "$target" "$corpus"
        log=$OUT/$target.log
        # -fork: each job a process of its own, so a crash or a leak in
        # one does not end the run, and the jobs share the corpus.
        # -timeout: an input that takes over 10 s is a hang - a loop a
        # parser must not have (libFuzzer's own limit is 20 minutes).
        "$BIN/$target" "$corpus" -fork="$jobs" -ignore_crashes=1 -ignore_timeouts=1 -timeout=10 -max_total_time="$seconds" \
            -artifact_prefix="$crashes/" -print_final_stats=1 > "$log" 2>&1
        found=$(ls "$crashes" | wc -l)
        edges=$(grep -o 'cov: [0-9]*' "$log" | tail -1)
        echo "fuzz: $target: ${seconds} s, ${edges:-cov: ?}, corpus $(ls "$corpus" | wc -l), crashes $found (log: $log)"
        if [ "$found" -gt 0 ]; then
            failed="$failed $target"
            # libFuzzer keeps every crashing input; one bug is often
            # thousands. Each place a panic happened, once, with an input.
            for input in $(ls -S "$crashes" | tail -50); do
                place=$(timeout 20 "$BIN/$target" -timeout=10 "$crashes/$input" 2>&1 | grep -a -m1 -A1 "panicked at\|ALARM: working on the last Unit for" | tr '\n' ' ' |
                        sed -e "s|.*panicked at $ROOT/||" -e 's/.*ALARM: working on the last Unit for \([0-9]*\) seconds.*/a hang (over 10 s)/' -e 's/(.*) //' -e 's/the len is [0-9]* but the index is [0-9]*/out of range/')
                echo "$place|$input"
            done | sort -t'|' -k1,1 -u | while IFS='|' read -r place input; do
                echo "    $place (smallest: $crashes/$input)"
            done
        fi
    done
    if [ -n "$failed" ]; then
        echo "fuzz: FAILED:$failed - the inputs are in $OUT/crashes/"
        exit 1
    fi
    ;;
*)
    sed -n '2,13p' "$0" | sed 's/^# \{0,1\}//'
    exit 2
    ;;
esac
