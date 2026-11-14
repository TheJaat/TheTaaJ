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
grep -q "\[selftest\] files:" "$LOG"              || fail "file service self-test"
grep -q "\[kernel\] idle" "$LOG"                  || fail "did not reach idle"
grep -q "\[init\] started:.* shell kbd" "$LOG" || fail "init did not start the services"
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
if [ -n "${DISK:-}" ]; then
    grep -q "\[pci\] .* virtio block device: .* handed to init" "$LOG" || fail "the kernel did not offer the disk"
    grep -q "\[blk\] virtio disk" "$LOG"           || fail "the disk driver did not start"
    if [ -n "${TFSTOOL:-}" ]; then
        # Proof that files reached the disk itself, in a form another
        # implementation of TaajFS reads: the host tool, on the image,
        # after QEMU is gone.
        [ "$("$TFSTOOL" get "$DISK" saved 2>&1)" = "kept" ] \
            || fail "tfstool does not find \"kept\" in the file \"saved\" on the disk"
        ! "$TFSTOOL" get "$DISK" notes > /dev/null 2>&1 || fail "the removed file \"notes\" is still on the disk"
        "$TFSTOOL" dump "$DISK" > "$LOG.dump" || fail "tfstool cannot read the volume the OS left"
    fi
fi
if [ -n "${EXPECT:-}" ]; then
    # The output of the commands typed: each pattern must appear.
    old_ifs=$IFS
    IFS=';'
    for pattern in $EXPECT; do
        grep -qE "$pattern" "$LOG" || fail "no output matching \"$pattern\""
    done
    IFS=$old_ifs
    # `ps` is typed before any program is run and again after the last:
    # the same processes, threads and channels both times, or running a
    # program left something behind - a handle the shell or init forgot.
    [ "$(grep -cE "^[0-9]+ processes, [0-9]+ threads, [0-9]+ channels" "$LOG")" -ge 2 ] \
        || fail "ps did not run twice"
    [ "$(grep -E "^[0-9]+ processes, [0-9]+ threads, [0-9]+ channels" "$LOG" | sort -u | wc -l)" -eq 1 ] \
        || fail "running programs left processes, threads or channels behind"
fi
sed -n '/Hello from TheTaaJ/,$p' "$LOG"
