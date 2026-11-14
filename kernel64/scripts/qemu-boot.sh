#!/bin/sh
# qemu-boot.sh - boots an ISO headless and stops as soon as the kernel is
# done, instead of waiting out a fixed timeout.
#
#   usage: qemu-boot.sh ISO LOG [SECONDS]
#
# The kernel never exits, so "done" is read from its serial output: the
# idle line, a panic, or a CPU exception. SECONDS (default 20) is only
# the limit for a kernel that hangs silently.
#
# Environment:
#   QEMU_ARGS  extra QEMU options (for example the firmware)
#   KEYS       QEMU key names to type once the kernel is idle, through
#              QEMU's monitor (`sendkey`) - e.g. "shift-h i ret". The
#              script then waits for the kernel to echo a line.

set -u

ISO=$1
LOG=$2
LIMIT=${3:-20}
KEYS=${KEYS:-}

FINISHED="\[kernel\] idle|KERNEL PANIC|CPU EXCEPTION"
MONITOR="$LOG.monitor"

rm -f "$LOG" "$MONITOR"
: > "$LOG"
qemu-system-x86_64 -m 512M -cdrom "$ISO" -boot d \
    -display none -no-reboot -serial "file:$LOG" \
    -monitor "unix:$MONITOR,server,nowait" ${QEMU_ARGS:-} &
qemu=$!

# Polls (ten times a second) until PATTERN appears in the log or the
# time runs out; true if it appeared.
wait_for() {
    pattern=$1
    ticks=$2
    while [ "$ticks" -gt 0 ]; do
        grep -qE "$pattern" "$LOG" && return 0
        kill -0 $qemu 2>/dev/null || return 1   # QEMU quit (a triple fault resets; -no-reboot quits)
        sleep 0.1
        ticks=$((ticks - 1))
    done
    return 1
}

if wait_for "$FINISHED" $((LIMIT * 10)); then
    if [ -n "$KEYS" ] && grep -q "\[kernel\] idle" "$LOG"; then
        # One key at a time, with a pause: a real typist, not a burst.
        for key in $KEYS; do
            echo "sendkey $key"
            sleep 0.05
        done | nc -U -N "$MONITOR" > /dev/null
        wait_for "\[keyboard\] line" 50 || echo "qemu-boot: no echoed line after typing" >&2
    fi
    sleep 0.3   # let the rest of a fault report reach the log
else
    echo "qemu-boot: no end of output after ${LIMIT}s - killed" >&2
fi

kill $qemu 2>/dev/null
wait $qemu 2>/dev/null
rm -f "$MONITOR"
exit 0
