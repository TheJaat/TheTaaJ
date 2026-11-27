#!/bin/sh
# miri-check.sh - the host tests under Miri (K187, journal entry NNN).
#
#   make miri        (or: sh tools/miri-check.sh [CRATE...])
#
# Miri runs a program in an interpreter that checks every memory access
# against Rust's rules: out of bounds, misaligned, use after free, a
# pointer made invalid by a later borrow (the aliasing rules), a data race
# between threads, memory never freed. It is how an `unsafe` block whose
# written reason (K112, make unsafe-check) is wrong gets caught.
#
# The crates: every one whose source has `unsafe` - found by looking, so a
# crate that gains some is checked from then on. A crate without any is
# skipped and said so: in safe Rust the compiler has ruled out what Miri
# looks for, and its normal tests check the rest. The kernel library is
# run for the host, as `make kernel64-test` runs it.
#
# Slow under Miri - it interprets: tests size themselves down with
# `cfg!(miri)` (fewer rounds, a smaller disk), and the few that cannot are
# `#[cfg_attr(miri, ignore = "why")]`. Not part of `make test64`: minutes
# to an hour, so a nightly job. A nightly Rust of a fixed date, which
# rustup installs with Miri the first time.

set -u
cd "$(dirname "$0")/.." || exit 1
NIGHTLY=nightly-2026-11-25
LIMIT=${MIRI_LIMIT:-1800}

rustup run "$NIGHTLY" cargo miri --version > /dev/null 2>&1 \
    || rustup toolchain install "$NIGHTLY" --profile minimal --component miri,rust-src > /dev/null \
    || { echo "miri-check: cannot install $NIGHTLY with Miri" >&2; exit 1; }

CRATES=${*:-kernel/kernel64 kernel/abi heap virtio taajfs fileserver net registry console input gfx gui ahci nvme usb e1000 acpi}
failed=0
printf '%-16s %-8s %s\n' crate seconds result
for dir in $CRATES; do
    [ -f "$dir/Cargo.toml" ] || continue
    if ! grep -rqw unsafe "$dir/src" --include='*.rs'; then
        printf '%-16s %-8s %s\n' "$dir" - "skipped: no unsafe"
        continue
    fi
    target=
    [ "$dir" = kernel/kernel64 ] && target="--target x86_64-unknown-linux-gnu"
    log=$(mktemp)
    start=$(date +%s)
    (cd "$dir" && timeout "$LIMIT" cargo "+$NIGHTLY" miri test --lib $target -- --test-threads=8) > "$log" 2>&1
    status=$?
    result=$(grep -a "^test result" "$log" | head -1 | sed 's/test result: //; s/; 0 measured.*//')
    if [ $status -ne 0 ]; then
        failed=1
        [ $status -eq 124 ] && result="timed out after $LIMIT s"
        result="FAILED: ${result:-see below}"
        grep -a -A4 "^error" "$log" | head -12 | sed 's/^/    /'
    fi
    printf '%-16s %-8s %s\n' "$dir" $(( $(date +%s) - start )) "$result"
    rm -f "$log"
done
exit $failed
