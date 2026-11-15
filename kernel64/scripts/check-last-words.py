#!/usr/bin/env python3
"""check-last-words.py - checks a QEMU screendump (binary PPM) shows the
kernel's last words: the whole screen painted over in the dark red of
lastwords.rs, with light text on it.

    usage: check-last-words.py SCREEN.ppm
"""
import sys


def read_ppm(path):
    with open(path, "rb") as f:
        data = f.read()
    fields, pos = [], 0
    while len(fields) < 4:
        while data[pos:pos + 1].isspace():
            pos += 1
        start = pos
        while not data[pos:pos + 1].isspace():
            pos += 1
        fields.append(data[start:pos])
    if fields[0] != b"P6" or fields[3] != b"255":
        sys.exit(f"{path}: not a binary 8-bit PPM")
    return int(fields[1]), int(fields[2]), data[pos + 1:]


def main():
    width, height, pixels = read_ppm(sys.argv[1])

    def at(x, y):
        o = 3 * (y * width + x)
        return tuple(pixels[o:o + 3])

    def near(got, want):
        return all(abs(g - w) <= 8 for g, w in zip(got, want))

    paper, ink = (0x58, 0x10, 0x18), (0xF0, 0xF0, 0xF0)
    # The corners and the bottom edge: text never reaches them, so they
    # are paper if - and only if - the whole screen was painted over.
    for x, y in [(2, 2), (width - 3, 2), (2, height - 3), (width - 3, height - 3), (width // 2, height - 3)]:
        if not near(at(x, y), paper):
            print(f"last words: ({x}, {y}) is {at(x, y)}, not the kernel's dark red - the screen was not taken over")
            return 1
    written = sum(1 for y in range(0, height, 2) for x in range(0, width, 2) if near(at(x, y), ink))
    if written < 300:
        print(f"last words: only {written} sampled pixels of text - nothing was written")
        return 1
    print(f"last words: on the screen ({written} sampled pixels of text)")


if __name__ == "__main__":
    sys.exit(main())
