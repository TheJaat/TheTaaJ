#!/bin/sh
# clean-build.sh - TheTaaJ built and tested from a fresh clone, on a clean
# Ubuntu 24.04, by its own setup script: one command (J92, journal entry 133).
#
#   sh tools/clean-build.sh [DIR [REF]]
#
# What a new machine would do, in a container made for the run and thrown
# away after it:
#   1. Ubuntu's own minimal 24.04 (ubuntu-base), its checksums' signature
#      checked with the archive key this machine already has;
#   2. unpacked fresh, and entered as root with unshare - a user namespace
#      (no sudo: root in the container is you outside, and the uids above
#      it are your /etc/subuid range, through newuidmap from `uidmap`), a
#      mount namespace and a PID namespace - with this machine's /dev, so
#      /dev/kvm works;
#   3. inside: tools/setup-wsl.sh, fetched from GitHub at REF - it installs the
#      packages and Rust, clones TheTaaJ over HTTPS with its submodules,
#      builds the i686 cross compiler and the ISO;
#   4. REF checked out (main unless told), and `make test64` - holding this
#      machine's test64 lock, as every session does (rule 13).
#
# DIR (default ~/.cache/taaj-clean-build) keeps the download, the packages
# apt fetched (so a second run does not fetch them again) and the last
# run's logs; the container's files are deleted after the run. Ubuntu
# 24.04 is the reference: its LLVM is the 18.1.3 the byte-for-byte tests
# compare with, so they need nothing unpacked here. It needs: unshare and
# newuidmap (uidmap), curl, gpgv, tar, and a subuid range for this user.
# Exits with make test64's status; the summary is the last lines.

set -eu
DIR=${1:-$HOME/.cache/taaj-clean-build}
REF=${2:-main}
REPO=https://github.com/TheJaat/TheTaaJ
BASE=https://cdimage.ubuntu.com/ubuntu-base/releases/24.04/release

say() { printf '\n== clean-build: %s\n' "$*"; }
mkdir -p "$DIR/dl" "$DIR/apt" "$DIR/logs"
DIR=$(cd "$DIR" && pwd)

command -v newuidmap > /dev/null || { echo "clean-build: no newuidmap - sudo apt install uidmap" >&2; exit 1; }
grep -q "^$(id -un):" /etc/subuid || { echo "clean-build: $(id -un) has no range in /etc/subuid" >&2; exit 1; }

# ---- 1. Ubuntu's base, checked ---------------------------------------------
say "Ubuntu 24.04's base image"
cd "$DIR/dl"
curl -sSfL -o SHA256SUMS "$BASE/SHA256SUMS"
curl -sSfL -o SHA256SUMS.gpg "$BASE/SHA256SUMS.gpg"
gpgv --keyring /usr/share/keyrings/ubuntu-archive-keyring.gpg SHA256SUMS.gpg SHA256SUMS 2> gpgv.out \
    || { cat gpgv.out >&2; echo "clean-build: the checksums' signature does not check" >&2; exit 1; }
IMAGE=$(grep -o 'ubuntu-base-24\.04\.[0-9]*-base-amd64\.tar\.gz' SHA256SUMS | sort -V | tail -1)
[ -f "$IMAGE" ] || curl -sSfL -o "$IMAGE" "$BASE/$IMAGE"
grep " \*$IMAGE\$" SHA256SUMS | sed 's/ \*/  /' | sha256sum -c --quiet || { echo "clean-build: $IMAGE does not match its checksum" >&2; exit 1; }
echo "  $IMAGE: signature and checksum good"

# ---- 2 to 4, in the container -------------------------------------------------
ROOTFS="$DIR/rootfs"
LOG="$DIR/logs/clean-build-$(date -u +%Y%m%d-%H%M%S).log"
say "a fresh container (log: $LOG)"

# Everything that runs as root in the user namespace. Deleting the old
# container's files needs it too: they belong to uids of the range.
unshare --map-auto --map-root-user --mount --pid --fork sh -eu -c '
    DIR=$1; ROOTFS=$2; IMAGE=$3; REF=$4; REPO=$5
    rm -rf "$ROOTFS"
    mkdir -p "$ROOTFS"
    tar -xzf "$DIR/dl/$IMAGE" -C "$ROOTFS"
    # The host'"'"'s devices (and /dev/kvm), a /proc of this PID namespace,
    # its network'"'"'s name servers, and the packages kept from last time.
    mount --rbind /dev "$ROOTFS/dev"
    mount -t proc proc "$ROOTFS/proc"
    mount --rbind /sys "$ROOTFS/sys"
    cp /etc/resolv.conf "$ROOTFS/etc/resolv.conf"
    mkdir -p "$ROOTFS/var/cache/apt/archives"
    mount --bind "$DIR/apt" "$ROOTFS/var/cache/apt/archives"
    # The machine'"'"'s test64 lock: one make test64 at a time, here too.
    touch /tmp/taaj-test64.lock "$ROOTFS/tmp/taaj-test64.lock"
    mount --bind /tmp/taaj-test64.lock "$ROOTFS/tmp/taaj-test64.lock"
    chroot "$ROOTFS" /usr/bin/env -i HOME=/root TERM=dumb PATH=/root/.cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
        REF="$REF" REPO="$REPO" /bin/bash -euc "
        # Keep the packages apt fetches, for the next run.
        echo \"Binary::apt::APT::Keep-Downloaded-Packages \\\"true\\\";\" > /etc/apt/apt.conf.d/01keep
        apt-get update -q > /dev/null
        apt-get install -y -q curl ca-certificates > /dev/null
        curl -sSfL -o /root/setup-wsl.sh \$REPO/raw/\$REF/tools/setup-wsl.sh
        bash /root/setup-wsl.sh /root/TheTaaJ
        cd /root/TheTaaJ
        git fetch -q origin \"\$REF\" && git checkout -q FETCH_HEAD && git submodule update -q --init --recursive
        echo \"clean-build: testing \$(git log --oneline -1)\"
        flock /tmp/taaj-test64.lock make test64
    "
' sh "$DIR" "$ROOTFS" "$IMAGE" "$REF" "$REPO" > "$LOG" 2>&1 && status=0 || status=$?

tail -5 "$LOG"
# The container's files - and the package cache's partial/, which apt's own
# user owns - as the user namespace's root again.
unshare --map-auto --map-root-user sh -c 'rm -rf "$1"; rm -rf "$2/partial"' sh "$ROOTFS" "$DIR/apt"
if [ $status -eq 0 ]; then
    say "a fresh clone of $REF builds and passes on a clean Ubuntu 24.04 (log: $LOG)"
else
    say "FAILED with status $status (log: $LOG)"
fi
exit $status
