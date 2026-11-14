#!/bin/sh
# check-boot.sh - checks a 64-bit kernel boot log for everything a good
# boot prints, and nothing a bad one does.
#
#   usage: check-boot.sh LOG [BIOS|UEFI]
#
# With a boot method, also checks the kernel saw that method - proof the
# boot info really came from that loader. With $TYPED set, checks the
# kernel echoed that line from the keyboard.

LOG=$1
METHOD=${2:-}
TYPED=${TYPED:-}

fail() {
    echo "FAIL: $1"
    cat "$LOG"
    exit 1
}

grep -q "Hello from TheTaaJ" "$LOG"                || fail "no greeting"
grep -q "resumed after the breakpoint" "$LOG"      || fail "exception path broken"
grep -q "\[selftest\] memory: all passed" "$LOG"   || fail "memory self-test"
grep -q "\[selftest\] timer" "$LOG"               || fail "timer self-test"
grep -q "\[selftest\] threads: sleep" "$LOG"      || fail "threads self-test"
grep -q "\[selftest\] user:" "$LOG"               || fail "user-mode self-test"
grep -q "\[selftest\] ipc:" "$LOG"                || fail "IPC self-test"
grep -q "\[kernel\] idle" "$LOG"                  || fail "did not reach idle"
! grep -qE "KERNEL PANIC|CPU EXCEPTION" "$LOG"     || fail "panic or exception"
if [ -n "$METHOD" ]; then
    grep -qE "loader +TheTaaJBoot.*\($METHOD\)" "$LOG" || fail "not booted via $METHOD"
fi
if [ -n "$TYPED" ]; then
    grep -qF "[keyboard] line: \"$TYPED\"" "$LOG"  || fail "keyboard: \"$TYPED\" not echoed"
fi
sed -n '/Hello from TheTaaJ/,$p' "$LOG"
