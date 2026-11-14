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
#   KEYS       QEMU key names to type once the keyboard driver is ready,
#              through QEMU's monitor (`sendkey`) - e.g. "shift-h i ret".
#              A "/" waits until a new keyboard driver is ready (after a
#              crash and restart) before typing on. The script then waits
#              for the shell to echo a line.

set -u

ISO=$1
LOG=$2
LIMIT=${3:-20}
KEYS=${KEYS:-}

FINISHED="\[kernel\] idle|KERNEL PANIC|CPU EXCEPTION"
MONITOR="$LOG.monitor"

rm -f "$LOG" "$LOG.ppm" "$LOG.hang.ppm" "$LOG.qemu" "$MONITOR"
: > "$LOG"
qemu-system-x86_64 -m 512M -cdrom "$ISO" -boot d \
    -display none -no-reboot -serial "file:$LOG" \
    -monitor "unix:$MONITOR,server,nowait" \
    -d cpu_reset -D "$LOG.qemu" ${QEMU_ARGS:-} &
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
    # The driver is a process that starts after the kernel goes idle, and
    # it flushes the controller as it sets it up: type only once it is
    # ready, or the first keys are lost.
    if [ -n "$KEYS" ] && grep -q "\[kernel\] idle" "$LOG" && wait_for "\[kbd\] PS/2 keyboard ready" 50; then
        # One key at a time, with a pause: a real typist, not a burst.
        ready="\[kbd\] PS/2 keyboard ready"
        for key in $KEYS; do
            if [ "$key" = "/" ]; then
                # Wait for one more "ready" line than there is now.
                seen=$(grep -c "$ready" "$LOG")
                ticks=100
                while [ "$(grep -c "$ready" "$LOG")" -le "$seen" ] && [ $ticks -gt 0 ]; do
                    sleep 0.1
                    ticks=$((ticks - 1))
                done
                continue
            fi
            echo "sendkey $key" | nc -U -N "$MONITOR" > /dev/null
            sleep 0.05
        done
        wait_for "\[shell\] line" 50 || echo "qemu-boot: no echoed line after typing" >&2
    fi
    sleep 0.3   # let the rest of a fault report reach the log
    # A picture of the screen, for check-boot.sh to look at.
    echo "screendump $LOG.ppm" | nc -U -N "$MONITOR" > /dev/null
    sleep 0.3
else
    if kill -0 $qemu 2>/dev/null; then
        echo "qemu-boot: no end of output after ${LIMIT}s - killed" >&2
        # What was on the screen: the BIOS loader reports only there.
        echo "screendump $LOG.hang.ppm" | nc -U -N "$MONITOR" > /dev/null
        sleep 0.3
    else
        # -no-reboot: a triple fault resets the CPU, and QEMU quits.
        # $LOG.qemu holds the CPU state at each reset (-d cpu_reset).
        echo "qemu-boot: QEMU quit before the kernel finished - a triple fault? (CPU state in $LOG.qemu)" >&2
    fi
fi

kill $qemu 2>/dev/null
wait $qemu 2>/dev/null
rm -f "$MONITOR"
exit 0
