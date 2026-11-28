#!/bin/sh
# boot-time.sh - how long a BIOS boot takes to reach the kernel: from
# starting QEMU to the kernel's first line on the serial port ("Hello from
# TheTaaJ!"). Everything before it is the firmware and the loader - stage
# 1, stage 2 reading the kernel through the BIOS (K131), the ELF loader,
# the switch to long mode.
#
#   usage: tools/boot-time.sh ISO [ACCEL [RUNS]]   ACCEL tcg (default) or kvm
#
# Prints each run's seconds and their median. One CPU, as the loader uses
# one; nothing attached but the ISO.

set -u
ISO=$1
ACCEL=${2:-tcg}
RUNS=${3:-3}
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

times=""
run=1
while [ $run -le "$RUNS" ]; do
    log="$WORK/serial.$run"
    : > "$log"
    start=$(date +%s.%N)
    qemu-system-x86_64 -accel "$ACCEL" -m 512M -cdrom "$ISO" -boot d -smp 1 \
        -display none -no-reboot -serial "file:$log" > /dev/null 2>&1 &
    qemu=$!
    found=""
    # Up to two minutes: an emulated BIOS boot took 14 s in entry 105.
    tries=2400
    while [ $tries -gt 0 ]; do
        if grep -aq 'Hello from TheTaaJ!' "$log"; then
            found=$(date +%s.%N)
            break
        fi
        sleep 0.05
        tries=$((tries - 1))
    done
    kill $qemu 2>/dev/null
    wait $qemu 2>/dev/null
    if [ -z "$found" ]; then
        echo "boot-time: run $run never reached the kernel" >&2
        exit 1
    fi
    seconds=$(echo "$found - $start" | bc)
    printf 'run %d: %.2f s\n' "$run" "$seconds"
    times="$times $seconds"
    run=$((run + 1))
done
median=$(echo $times | tr ' ' '\n' | sort -n | sed -n "$(( (RUNS + 1) / 2 ))p")
printf 'boot-time (%s): median %.2f s over %d runs\n' "$ACCEL" "$median" "$RUNS"
