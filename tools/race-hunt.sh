#!/bin/bash
# race-hunt.sh - runs test64's parts again and again, to find what fails
# only sometimes (journal entry 105, MaK's item K19; J95, entry 156).
#
#   usage: tools/race-hunt.sh [ROUNDS] [FIRST]
#          (defaults: 20 rounds, from the round after the last one in
#          build/race-hunt/summary.txt - so a hunt spread over several
#          nights goes on where it stopped)
#
#   ALL=1          every part of test64; by default only the x86_64 ones,
#                  the ones KVM runs at full speed (AArch64 is emulated
#                  everywhere, and is the slowest part of test64)
#   ONLY="A B"     only these parts - one flaky part, 50 times, say
#   TAAJ_ACCEL=    kvm or tcg, as for make; the Makefile's choice if unset
#
# Run `make test64-shared` once first (it builds what the parts use).
# Each round's make output is build/race-hunt/round-N.log, and
# build/race-hunt/summary.txt gets a line per round naming the parts that
# failed. When a round fails, every file in build/ written during it -
# the boots' serial logs, QEMU's own logs, the screenshots, the TCP
# tests' packets - is copied to build/race-hunt/round-N/ before the next
# round starts and overwrites them: a failure seen once must be readable
# afterwards.
#
# As make test64 runs them (J95): TEST_JOBS at a time, the longest first
# by build/test64-times, and each round under the machine's test64 lock
# (rule 13) - taken per round, so a session's own test can come between
# two rounds. At the end, build/race-hunt/tally.txt: each part that ever
# failed, in how many of all the rounds in summary.txt.
set -u
ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT" || exit 1
D=build/race-hunt
mkdir -p "$D"
ROUNDS=${1:-20}
LAST=$(sed -n 's/^round \([0-9]*\):.*/\1/p' "$D/summary.txt" 2>/dev/null | sort -n | tail -1)
FIRST=${2:-$(( ${LAST:-0} + 1 ))}
PARTS=$(make -s --no-print-directory test64-list)
[ -n "${ONLY:-}" ] && PARTS=$ONLY
[ "${ALL:-0}" = 1 ] || [ -n "${ONLY:-}" ] || PARTS=$(echo "$PARTS" | tr ' ' '\n' | grep -v '^aarch64-' | tr '\n' ' ')
TIMES=build/test64-times
if [ -s "$TIMES" ]; then
    PARTS=$(for p in $PARTS; do echo "$p"; done | awk -v times="$TIMES" '
        BEGIN { while ((getline line < times) > 0) { split(line, f, " "); took[f[1]] = f[2] } }
        { print ($1 in took ? took[$1] : 999999), NR, $1 }' | sort -k1,1nr -k2,2n | awk '{ printf "%s ", $3 }')
fi
ACCEL=${TAAJ_ACCEL:+TAAJ_ACCEL=$TAAJ_ACCEL}
# What make test64-shared built is not built again each round (make -o,
# as test64.sh does): most of it is .PHONY - LLVM's tools relinked every
# round otherwise (entry 117).
SKIP=$(make -s --no-print-directory --eval 'race-hunt-shared: ; @echo $(TEST64_SHARED) smp64-test' race-hunt-shared | sed 's/[^ ][^ ]*/-o &/g')
echo "race-hunt: rounds $FIRST-$((FIRST + ROUNDS - 1)) of $(echo $PARTS | wc -w) parts at $(git rev-parse --short HEAD) ${ACCEL:-(accelerator chosen by the Makefile)}" >> "$D/summary.txt"
for i in $(seq "$FIRST" $((FIRST + ROUNDS - 1))); do
    log=$D/round-$i.log
    flock /tmp/taaj-test64.lock bash -c '
        touch "$1/.round-start"
        date +%s > "$1/.round-began"
        make -k --no-print-directory -j"${TEST_JOBS:-12}" --output-sync=target $5 TEST64_PARTS="$2" $3 test64-parts > "$4" 2>&1
        echo $? > "$1/.round-code"' - "$D" "$PARTS" "$ACCEL" "$log" "$SKIP"
    code=$(cat "$D/.round-code")
    failed=$(for p in $PARTS; do grep -q "\[Makefile:[0-9]*: $p\] Error" "$log" && printf '%s ' "$p"; done)
    if [ -n "$failed" ]; then
        mkdir -p "$D/round-$i"
        find build -maxdepth 2 -type f -newer "$D/.round-start" -not -path "$D/*" -size -64M \
            -exec cp -a {} "$D/round-$i/" \; 2>/dev/null
    fi
    echo "round $i: exit $code, $(( $(date +%s) - $(cat "$D/.round-began") )) s, failed: ${failed:-none}" >> "$D/summary.txt"
    grep -h "^FAIL\|KERNEL PANIC\|panicked at" "$log" | sort | uniq -c | head -5 | sed 's/^/    /' >> "$D/summary.txt"
done
echo "race-hunt: done" >> "$D/summary.txt"

# Every round so far (a round run twice counts once: its last line).
awk '/^round [0-9]+:/ { n = $2; sub(":", "", n); sub(/.*failed: /, ""); last[n] = $0 }
     END { for (n in last) { rounds++; if (last[n] != "none") { k = split(last[n], f, " "); for (j = 1; j <= k; j++) bad[f[j]]++ } }
           print "race-hunt: " rounds " rounds in all"
           for (p in bad) printf "%s failed in %d of %d rounds\n", p, bad[p], rounds }' "$D/summary.txt" |
    { read -r head; echo "$head"; sort -k4,4nr; } > "$D/tally.txt"
cat "$D/tally.txt"
