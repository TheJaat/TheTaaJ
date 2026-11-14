#!/bin/sh
# check-boot.sh - checks a 64-bit kernel boot log for everything a good
# boot prints, and nothing a bad one does.
#
#   usage: check-boot.sh LOG [BIOS|UEFI]
#
# With a boot method, also checks the kernel saw that method - proof the
# boot info really came from that loader.

LOG=$1
METHOD=${2:-}

fail() {
    echo "FAIL: $1"
    cat "$LOG"
    exit 1
}

grep -q "Hello from TheTaaJ" "$LOG"                || fail "no greeting"
grep -q "resumed after the breakpoint" "$LOG"      || fail "exception path broken"
grep -q "\[selftest\] memory: all passed" "$LOG"   || fail "memory self-test"
grep -q "Nothing more to do yet" "$LOG"            || fail "did not finish"
! grep -qE "KERNEL PANIC|CPU EXCEPTION" "$LOG"     || fail "panic or exception"
if [ -n "$METHOD" ]; then
    grep -qE "loader +TheTaaJBoot.*\($METHOD\)" "$LOG" || fail "not booted via $METHOD"
fi
sed -n '/Hello from TheTaaJ/,$p' "$LOG"
