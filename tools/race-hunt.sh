#!/bin/bash
# race-hunt.sh - runs test64's parts again and again, to find what fails
# only sometimes (journal entry 105, MaK's item K19).
#
#   usage: tools/race-hunt.sh [ROUNDS] [FIRST]     (defaults: 20 rounds, from 1)
#
#   ALL=1          every part of test64; by default only the x86_64 ones,
#                  the ones KVM runs at full speed (AArch64 is emulated
#                  everywhere, and is the slowest part of test64)
#   TAAJ_ACCEL=    kvm or tcg, as for make; the Makefile's choice if unset
#
# Run `make test64-shared` once first (it builds what the parts use).
# Each round's make output is build/race-hunt/round-N.log, and
# build/race-hunt/summary.txt gets a line per round naming the parts that
# failed. When a round fails, every file in build/ written during it -
# the boots' serial logs, QEMU's own logs, the screenshots - is copied to
# build/race-hunt/round-N/ before the next round starts and overwrites
# them: a failure seen once must be readable afterwards.
set -u
ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT" || exit 1
ROUNDS=${1:-20}
FIRST=${2:-1}
D=build/race-hunt
mkdir -p "$D"
PARTS=$(sed -n 's/^TEST64_PARTS = //p' Makefile)
[ "${ALL:-0}" = 1 ] || PARTS=$(echo "$PARTS" | tr ' ' '\n' | grep -v '^aarch64-' | tr '\n' ' ')
ACCEL=${TAAJ_ACCEL:+TAAJ_ACCEL=$TAAJ_ACCEL}
echo "race-hunt: rounds $FIRST-$((FIRST + ROUNDS - 1)) of $(echo $PARTS | wc -w) parts ${ACCEL:-(accelerator chosen by the Makefile)}" >> "$D/summary.txt"
for i in $(seq "$FIRST" $((FIRST + ROUNDS - 1))); do
    start=$(date +%s)
    log=$D/round-$i.log
    touch "$D/.round-start"
    make -k --no-print-directory -j"${TEST_JOBS:-12}" --output-sync=target TEST64_PARTS="$PARTS" $ACCEL test64-parts > "$log" 2>&1
    code=$?
    failed=$(for p in $PARTS; do grep -q "\[Makefile:[0-9]*: $p\] Error" "$log" && printf '%s ' "$p"; done)
    if [ -n "$failed" ]; then
        mkdir -p "$D/round-$i"
        find build -maxdepth 2 -type f -newer "$D/.round-start" -not -path "$D/*" -size -64M \
            -exec cp -a {} "$D/round-$i/" \; 2>/dev/null
    fi
    echo "round $i: exit $code, $(( $(date +%s) - start )) s, failed: ${failed:-none}" >> "$D/summary.txt"
    grep -h "^FAIL\|KERNEL PANIC\|panicked at" "$log" | sort | uniq -c | head -5 | sed 's/^/    /' >> "$D/summary.txt"
done
echo "race-hunt: done" >> "$D/summary.txt"
