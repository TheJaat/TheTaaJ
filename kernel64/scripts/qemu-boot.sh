#!/bin/sh
# qemu-boot.sh - boots an ISO headless and stops as soon as the kernel is
# done, instead of waiting out a fixed timeout.
#
#   usage: qemu-boot.sh ISO LOG [SECONDS]
#
# The kernel halts rather than exiting, so "done" is read from its serial
# output: the normal last line, a panic, or a CPU exception. SECONDS
# (default 20) is only the limit for a kernel that hangs silently.
# Extra QEMU options (for example the firmware) come from $QEMU_ARGS.

set -u

ISO=$1
LOG=$2
LIMIT=${3:-20}

# How the kernel signals it has nothing more to say.
FINISHED="Nothing more to do yet|KERNEL PANIC|CPU EXCEPTION"

rm -f "$LOG"
: > "$LOG"
qemu-system-x86_64 -m 512M -cdrom "$ISO" -boot d \
    -display none -no-reboot -serial "file:$LOG" ${QEMU_ARGS:-} &
qemu=$!

# Poll ten times a second. After a match, wait a moment more so the rest
# of a fault report (the register dump) reaches the log too.
ticks=0
while [ $ticks -lt $((LIMIT * 10)) ]; do
    if grep -qE "$FINISHED" "$LOG"; then
        sleep 0.3
        break
    fi
    if ! kill -0 $qemu 2>/dev/null; then
        break   # QEMU exited by itself (a triple fault resets, and -no-reboot quits)
    fi
    sleep 0.1
    ticks=$((ticks + 1))
done

kill $qemu 2>/dev/null
wait $qemu 2>/dev/null
[ $ticks -lt $((LIMIT * 10)) ] || echo "qemu-boot: no end of output after ${LIMIT}s - killed" >&2
exit 0
