#!/bin/sh
# fault-test.sh - builds the kernel once per fault-* feature, boots each
# build headless, and checks the fault was caught and reported correctly.
#
#   usage: fault-test.sh STAGE1_BIN STAGE2_BIN BUILD_DIR [test...]
#
# A protection that silently fails to fault is exactly the kind of bug
# nothing else would notice, so each one gets a boot of its own.

set -eu

STAGE1=$1
STAGE2=$2
BUILD=$3
shift 3
TESTS=${*:-page double rodata nx stack}

HERE=$(cd "$(dirname "$0")/.." && pwd)

# What each test must print. grep -E patterns.
expected() {
    case $1 in
        page)   echo "kernel read at 0x4000000000: page not present" ;;
        double) echo "CPU EXCEPTION 8: double fault" ;;
        rodata) echo "kernel write at 0x[0-9a-f]+: protection violation" ;;
        nx)     echo "kernel instruction fetch at 0x[0-9a-f]+: protection violation" ;;
        stack)  echo "kernel stack overflow" ;;
        *)      echo "unknown fault test '$1'" >&2; exit 2 ;;
    esac
}

failed=0
for test in $TESTS; do
    pattern=$(expected "$test")
    target="$HERE/target/fault-$test"   # separate, so builds don't evict each other
    iso_dir="$BUILD/fault64-$test"
    iso="$BUILD/fault64-$test.iso"
    log="$BUILD/fault64-$test.log"

    (cd "$HERE" && cargo build --release --quiet --features "fault-$test" --target-dir "$target")

    rm -rf "$iso_dir"
    mkdir -p "$iso_dir/kernel"
    cp "$STAGE1" "$STAGE2" "$iso_dir/"
    strip --strip-debug -o "$iso_dir/kernel/kernel.elf" \
        "$target/x86_64-unknown-none/release/kernel64"
    xorriso -as mkisofs -R -J -b stage1.bin -iso-level 3 -no-emul-boot \
        -boot-load-size 4 -o "$iso" "$iso_dir" 2>/dev/null

    "$HERE/scripts/qemu-boot.sh" "$iso" "$log" 15

    if grep -qE "$pattern" "$log" && ! grep -q "\[test\] FAILED" "$log"; then
        echo "fault-$test: ok ($(grep -oE "$pattern" "$log" | head -1))"
    else
        echo "fault-$test: FAILED - expected /$pattern/"
        sed -n '/\[test\]/,$p' "$log"
        failed=1
    fi
done

exit $failed
