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
grep -q "\[init\] started: shell kbd" "$LOG"    || fail "init did not start the services"
if grep -q "framebuffer: .*handed to init" "$LOG"; then
    grep -q "\[display\] painted" "$LOG"            || fail "the display service did not paint"
    [ -f "$LOG.ppm" ]                                || fail "no screenshot"
    python3 -I "$(dirname "$0")/check-screen.py" "$LOG.ppm" || fail "the screen is not what the display painted"
fi
! grep -qE "KERNEL PANIC|CPU EXCEPTION" "$LOG"     || fail "panic or exception"
if [ -n "$METHOD" ]; then
    grep -qE "loader +TheTaaJBoot.*\($METHOD\)" "$LOG" || fail "not booted via $METHOD"
fi
if [ -n "$TYPED" ]; then
    grep -q "\[kbd\] PS/2 keyboard ready" "$LOG"     || fail "keyboard driver did not start"
    grep -qF "[shell] line: \"$TYPED\"" "$LOG"     || fail "keyboard: \"$TYPED\" not echoed by the shell"
    grep -q "\[init\] kbd ended .* restarting" "$LOG" || fail "init did not restart the crashed driver"
fi
sed -n '/Hello from TheTaaJ/,$p' "$LOG"
