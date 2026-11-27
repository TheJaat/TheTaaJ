#!/bin/sh
# test64.sh [again] - make test64's steps, and make test64-again (journal
# entry 137). Run by the Makefile, which gives it MAKE, TEST_JOBS,
# TEST64_SHARED and TEST64_PARTS.
#
#   make test64         1. the shared builds (test64-shared);
#                       2. smp64-test alone (its 20-CPU boot needs every
#                          boot slot, entry 117);
#                       3. every other part, in parallel.
#                       A failing part does not stop the others - nor does
#                       smp64-test the parts after it: one run reports
#                       every failure. Only a failed build stops it: the
#                       parts need it.
#   make test64-again   only what failed in the last run, written to
#                       build/test64-failed: after a fix that only that
#                       part's test can see (its script, its expected
#                       text, its own program). A fix to the kernel, libc
#                       or kernel/abi changes what every part boots - then
#                       make test64 (rule 15's question: what did the
#                       change touch?).
#
# build/test64-failed has the parts that failed, and those that never ran
# because a build they needed failed; `test64-parts` if make's output
# could not be read (then every part again); `test64-shared` when the
# shared builds failed (then all of make test64).
set -u
mkdir -p build
FAILED=build/test64-failed
LOG=build/test64-parts.log

smp=yes
parts=$TEST64_PARTS
if [ "${1:-}" = again ]; then
    last=$(cat "$FAILED" 2>/dev/null)
    if [ -z "$last" ]; then
        echo "test64-again: nothing failed in the last make test64 ($FAILED is empty)"
        exit 0
    fi
    case " $(echo $last) " in
    *" test64-shared "*) ;;  # all of it
    *" test64-parts "*) case " $(echo $last) " in *" smp64-test "*) ;; *) smp=no ;; esac ;;
    *)
        smp=no
        parts=
        for part in $last; do
            if [ "$part" = smp64-test ]; then smp=yes; else parts="$parts $part"; fi
        done
        ;;
    esac
    echo "test64-again: $(echo $last)"
fi

if ! $MAKE --no-print-directory test64-shared; then
    echo test64-shared > "$FAILED"
    echo "test64: FAILED: the shared builds - nothing was tested"
    exit 1
fi
: > "$FAILED"

if [ $smp = yes ]; then
    $MAKE --no-print-directory smp64-test || echo smp64-test >> "$FAILED"
fi

if [ -n "$(echo $parts)" ]; then
    skip=
    for target in $TEST64_SHARED smp64-test; do skip="$skip -o $target"; done
    { $MAKE --no-print-directory -k -j"$TEST_JOBS" --output-sync=target $skip $parts 2>&1
      echo $? > "$LOG.status"; } | tee "$LOG"
    if [ "$(cat "$LOG.status")" != 0 ]; then
        # The parts that failed, and those a failed build stopped (make
        # -k: "Target 'X' not remade because of errors") - at the parts'
        # own level: deeper ones are a part's own sub-makes.
        found=no
        for target in $(sed -n -e "s/^make\[$MAKELEVEL\]: \*\*\* \[[^]]*: \([^]]*\)\] Error.*/\1/p" \
                                -e "s/^make\[$MAKELEVEL\]: Target '\([^']*\)' not remade because of errors.*/\1/p" "$LOG"); do
            case " $(echo $parts) " in
            *" $target "*) echo "$target" >> "$FAILED"; found=yes ;;
            esac
        done
        if [ $found = no ]; then  # not understood: all of them again
            if [ "$parts" = "$TEST64_PARTS" ]; then echo test64-parts >> "$FAILED"
            else for part in $parts; do echo "$part" >> "$FAILED"; done
            fi
        fi
    fi
    rm -f "$LOG.status"
fi

if [ -s "$FAILED" ]; then
    sort -u "$FAILED" -o "$FAILED"
    echo "test64: FAILED: $(echo $(cat "$FAILED")) - after fixing, make test64-again runs only these (if the fix touched only them)"
    exit 1
fi
echo "test64: all passed"
