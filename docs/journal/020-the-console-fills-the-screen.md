# The Console Fills the Screen

*Journal entry #20 · a layout left over from the splash screen*

This one is small, and it came from using the system rather than from the roadmap. Looking at a running TheTaaJ, the top third of the screen was empty: a gradient, then the amber title band a third of the way down, then the console squeezed into what was left — 20 rows of text on a 768-pixel screen.

It looked like a rendering bug. It was not. It was a decision that had outlived its reason.

---

## Where the layout came from

When the display service first appeared (entry #11), the screen showed no text at all. It was a splash: a gradient with a band and the name on it, placed a third of the way down because that looked right for a picture. Then the console was added *below the band*, and nobody — I — moved the band.

```rust
fn band(&self) -> core::ops::Range<u32> {
    self.fb.height / 3..self.fb.height / 3 + self.fb.height / 12
}
```

Every later step measured from that line: the console's top edge is "the end of the band, plus a margin", and the number of rows is whatever fits below it.

## The change

The band moves to the top of the screen:

```rust
fn band(&self) -> core::ops::Range<u32> {
    0..self.fb.height / 12
}
```

That is the whole change in the display service. Because the console's position and its row count are *derived* from the band, nothing else needed touching: the console now starts just under the title and has **33 rows instead of 20**, at the same 80 columns.

It is worth noticing why a one-line change was enough. The background is a function of the row alone (entry #11), the console's top is computed from the band, and the grid is sized from the space that remains. No number was written down twice.

## The test had the old layout written into it

One place *had* written it down twice: the screenshot check. `check-screen.py` sampled a pixel "in the band" at a third of the height, sampled the top-left of the gradient at row 0, and counted text only below the old band. With the band at the top, row 0 is amber, so the check failed — correctly, since it was checking the old picture.

It now samples the band near the top, the gradient just below it, and counts text from the band's end downward.

That a test encodes the layout is not a flaw. A screenshot check has to know what the picture should be. But it does mean a deliberate change of appearance is always two edits: the picture, and the description of the picture.

## What I learned

- **Using the system finds what tests do not.** Every automated check passed with a third of the screen wasted, because the checks described the layout as it was, not as it should be.
- **Derive, don't repeat.** The part of the code where sizes were derived needed one line. The part where they were repeated needed three.

---

## Testing note

```
make run64
```

| Look for / type | You should see |
|---|---|
| The QEMU window at boot | The amber `TheTaaJ` band at the very top, the welcome line directly under it, no empty area above |
| `help` | The whole command list fits on one screen without scrolling |
| `ls` repeated until the screen is full | Text reaches close to the bottom edge, then scrolls; the band never moves or gets overwritten |
| `clear` | An empty console, the prompt directly under the band |

In the terminal log: `[display] painted the 1024x768 screen from ring 3 (…); console 80x33`.

Automated: `make test64` — the boot tests take a screenshot and check the band, the gradient and the text.
