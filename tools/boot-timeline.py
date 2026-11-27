#!/usr/bin/env python3
"""boot-timeline.py - where a boot's time goes (K23).

Boots an ISO in QEMU and stamps each line of the serial port with the
seconds since QEMU started, as it appears, until the shell says it is
ready (or a time limit). The kernel's log has no time of its own on each
line; stamped from outside, the gaps between lines show what is slow.

    usage: tools/boot-timeline.py ISO [--accel kvm|tcg] [--smp N] [--disk IMAGE]
                                      [--until TEXT] [--limit SECONDS]

Prints every line with its time, then the ten biggest gaps: the line
after each gap is what was waited for. Lines come from a file QEMU
writes; it is read every 5 ms, so a stamp can be late by that much.
"""

import argparse
import os
import subprocess
import sys
import tempfile
import time


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("iso")
    parser.add_argument("--accel", default="kvm")
    parser.add_argument("--smp", type=int, default=4)
    parser.add_argument("--disk", help="a TaajFS disk, attached as virtio as the tests do")
    parser.add_argument("--until", default="[shell] ready", help="the line that ends the boot")
    parser.add_argument("--limit", type=float, default=120.0)
    parser.add_argument("--quiet", action="store_true", help="only the gaps and the total")
    args = parser.parse_args()

    with tempfile.TemporaryDirectory() as work:
        log = os.path.join(work, "serial")
        open(log, "w").close()
        command = [
            "qemu-system-x86_64", "-accel", args.accel, "-m", "512M",
            "-cdrom", args.iso, "-boot", "d", "-smp", str(args.smp),
            "-display", "none", "-no-reboot", "-serial", "file:" + log,
        ]
        if args.disk:
            command += ["-drive", "file=%s,if=virtio,format=raw,snapshot=on" % args.disk]
        start = time.monotonic()
        qemu = subprocess.Popen(command, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        stamped = []
        partial = b""
        done = False
        try:
            with open(log, "rb") as serial:
                while time.monotonic() - start < args.limit and not done:
                    chunk = serial.read()
                    if not chunk:
                        if qemu.poll() is not None:
                            break
                        time.sleep(0.005)
                        continue
                    now = time.monotonic() - start
                    partial += chunk
                    *lines, partial = partial.split(b"\n")
                    for line in lines:
                        text = line.decode("utf-8", "replace").rstrip("\r")
                        stamped.append((now, text))
                        if args.until in text:
                            done = True
        finally:
            qemu.kill()
            qemu.wait()

    if not args.quiet:
        for at, text in stamped:
            print("%7.3f  %s" % (at, text))
    if not stamped:
        print("boot-timeline: nothing on the serial port", file=sys.stderr)
        return 1
    gaps = sorted(
        ((stamped[i][0] - (stamped[i - 1][0] if i else 0.0), i) for i in range(len(stamped))),
        reverse=True,
    )[:10]
    print("\nthe biggest gaps (the line after each is what was waited for):")
    for gap, i in gaps:
        before = stamped[i - 1][1] if i else "(QEMU started)"
        print("  %6.3f s  before %-60.60s  after %.60s" % (gap, stamped[i][1], before))
    end = stamped[-1][0]
    print("%s: %.3f s%s" % (args.accel, end, "" if done else " (limit or end: '%s' not seen)" % args.until))
    return 0 if done else 1


if __name__ == "__main__":
    sys.exit(main())
