"""check-screen.py - checks a QEMU screendump (binary PPM) shows what the
display service paints: a navy-to-teal gradient with an amber band a
third of the way down, and the console's text below it.

    usage: check-screen.py SCREEN.ppm

Exits 0 if it matches, 1 (with the reason) if not. The display service
runs in ring 3, so a correct screen proves the whole path: the kernel
made the framebuffer a memory object, init passed it on, the service
mapped it and wrote the pixels.
"""
import sys


def read_ppm(path):
    with open(path, "rb") as f:
        data = f.read()
    # Header: "P6" width height maxval, separated by whitespace, then one
    # whitespace byte, then width * height RGB triples.
    fields, pos = [], 0
    while len(fields) < 4:
        while data[pos:pos + 1].isspace():
            pos += 1
        start = pos
        while not data[pos:pos + 1].isspace():
            pos += 1
        fields.append(data[start:pos])
    if fields[0] != b"P6" or fields[3] != b"255":
        raise ValueError("not an 8-bit binary PPM")
    width, height = int(fields[1]), int(fields[2])
    return width, height, data[pos + 1:]


def main():
    width, height, pixels = read_ppm(sys.argv[1])

    def at(x, y):
        o = 3 * (y * width + x)
        return tuple(pixels[o:o + 3])

    checks = [
        ("the amber band", at(width // 2, height // 3 + height // 24), (0xE8, 0xA8, 0x38)),
        ("the top of the gradient", at(width // 2, 0), (0x10, 0x18, 0x30)),
        ("the bottom of the gradient", at(width // 2, height - 1), (0x10, 0x18 + 254 // 6, 0x30 + 254 // 4)),
    ]
    for what, got, want in checks:
        if any(abs(g - w) > 8 for g, w in zip(got, want)):
            print(f"screen: {what} is {got}, expected about {want}")
            return 1
    # Text: the console's light grey-blue, below the band. Some hundreds
    # of pixels of it at least - the welcome lines and the typed line.
    text = (0xE0, 0xE8, 0xF0)
    top = height // 3 + height // 12
    ink = sum(
        1
        for y in range(top, height, 2)
        for x in range(0, width, 2)
        if all(abs(g - w) <= 8 for g, w in zip(at(x, y), text))
    )
    if ink < 200:
        print(f"screen: only {ink} sampled pixels of text below the band - no console?")
        return 1
    print(f"screen: {width}x{height}, painted as expected, with text ({ink} sampled pixels)")
    return 0


sys.exit(main())
