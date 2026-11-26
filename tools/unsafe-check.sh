#!/bin/sh
# unsafe-check.sh - every `unsafe` says why it is sound (K112).
#
#   make unsafe-check      (or: USER_A64_BINS="..." sh tools/unsafe-check.sh)
#
# Runs clippy with two of its lints made errors, and no other:
#   - undocumented_unsafe_blocks: an `unsafe` block, or `unsafe impl`,
#     needs a `// SAFETY:` comment just above it - what makes it sound;
#   - missing_safety_doc: a public `unsafe fn` needs a `# Safety`
#     section - what its caller must promise.
# Over every crate the system is built from, for each CPU it is built
# for: the kernel, the programs (user/) and the C library for both, and
# the crates with logic tested on the host. Exits non-zero, naming each
# place, if any is missing (journal entry NNN).

set -u
cd "$(dirname "$0")/.." || exit 1

A64=aarch64-unknown-none-softfloat
# One Rust for all of them, the one the kernel, user/ and libc/ pin: the
# host crates otherwise get the machine's default, which differs between
# machines, and may have no clippy.
RUST=$(sed -n 's/^channel = "\(.*\)"/\1/p' kernel/kernel64/rust-toolchain.toml)
LINTS="-A clippy::all -D clippy::undocumented_unsafe_blocks -D clippy::missing_safety_doc"
failed=0

lint() { # lint DIR [cargo arguments]: one crate, one target
    dir=$1
    shift
    out=$(cd "$dir" && cargo "+$RUST" clippy --release --quiet "$@" -- $LINTS 2>&1)
    if [ $? -ne 0 ]; then
        echo "$out" | grep -A4 "^error" | grep -E "^error|-->" | sed "s|-->  *|  $dir/|"
        failed=1
    fi
}

for dir in kernel/kernel64 libc; do
    lint "$dir"
    lint "$dir" --target "$A64"
done
# user/ for AArch64: only the programs built for it (the Makefile's
# USER_A64_BINS) - the PC's port programs are not.
lint user
lint user --target "$A64" $(for bin in ${USER_A64_BINS:?run it as make unsafe-check}; do printf -- '--bin %s ' "$bin"; done)
for dir in kernel/abi heap virtio taajfs fileserver net gui gfx input console ahci nvme usb e1000 registry acpi; do
    [ -f "$dir/Cargo.toml" ] && lint "$dir" --lib
done

if [ $failed -eq 0 ]; then
    echo "unsafe-check: every unsafe block, impl and public unsafe fn says why"
fi
exit $failed
