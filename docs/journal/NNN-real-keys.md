# Real keys: one keyboard, its layout and its repeat, in the display

*Journal entry #NNN · a step, MaK-WSL/desktop's K116. Written on MaK-WSL/desktop. Each keyboard driver used to make the terminal's bytes itself, with the US layout built in, and each repeated a held key its own way - or not at all. Now the display turns every keyboard's key events into characters, in a layout chosen while running (`display layout dvorak`); repeats a held key the same way for all of them; and tells a window of a key coming up only if it was told of it going down.*

## Why

Entry 151 (K42) gave windows the keys themselves - codes, down and up, modifiers - beside the bytes. What K116 still asked for: repeat, a layout chosen in user space, the terminal's bytes made from the events, and whether an up never seen down is held back. Before this step:

- **Three copies of the layout.** kbd (PS/2), hid (virtio) and usb each fed the shared decoder and sent bytes, US only. Choosing another layout meant telling three drivers, and a new driver had to remember to do it too.
- **Three kinds of repeat.** A PS/2 keyboard repeats by itself (it sends the key down again). usb had a timer of its own. hid dropped the virtio keyboard's repeats: on AArch64 a held key typed once.
- **A stray up.** Entry 151's first log: the Enter that starts `counter` goes down in the terminal; `counter`'s window opens and takes the focus; `counter` is told "key 28 up" for a key it never saw go down.

## The decisions (the user's, 2026-11-28)

| Question | Chosen | Not chosen |
|---|---|---|
| Where the layout, repeat and bytes live | **In the display**, once for every keyboard; drivers send only key events to it | Each driver, with a shared setting: three places to keep in step, no switch while running |
| An up whose down a window did not see | **Held back**: an up goes to the window that was told of the down, and nowhere else | Every up passed on, true but surprising |

## How it works

```
 PS/2 ─ kbd ─┐  KEY (code, down/up)        ┌─ KeyCode ─▶ the window that heard the down
 virtio ─ hid ┼──────────────────▶ display ┤
 USB ─ usb ──┘  bytes: dropped    Keyboard  └─ bytes ───▶ the focus's window, or the shell
                (the serial        layout,
                 terminal's)       repeat
```

- **`input::keyboard::Keyboard`** (host-tested): a key code goes down or up, at a tick. It keeps the keys down (one bit each, codes to 0x2FF); a down for a key already down - the keyboard's own repeat - is dropped; the rest goes through the decoder (set-1 bytes from `evdev::scancode`, so Shift, Caps Lock and Control stay in one place) and comes out as a `Press`: the code, down or up, whether it is a repeat, and the terminal's bytes.
- **Repeat**: the newest key down that is not a modifier repeats after 50 ticks, then every 4 - half a second, then 25 a second, what usb did (`usb::Repeat`, reused). Its up ends it; an older key still held does not start again, as on other desktops. The display waits for its messages *or* the next repeat (`object_wait_until`), and a repeat is told to the window as the key going down again (Linux's value 2), with its bytes.
- **Layouts**: `Decoder` takes a `Layout` - US, and Dvorak as the second, because it is ASCII only, as the terminal's font is (a layout with letters beyond ASCII waits for K43's font). The shell's `display layout NAME` checks the name and sends `abi::display::LAYOUT`; `display layout` alone lists them.
- **Who is told of an up**: the display remembers, per key code, the window its down went to (`key_window`; 0 for the terminal or nobody). The up goes there, even if the focus moved since, and the entry is cleared. A repeat goes there too, so the window told of the repeats is the one told of the up.
- **The drivers' bytes** still flow - in the serial terminal (no screen) the shell reads them directly, and usb runs there too - but the display drops them: with a screen, every byte comes from the key events. The drivers are unchanged, but for their comments.

## Bugs

**1. A test of mine: the Caps Lock check.** `modifiers_do_not_repeat` pressed every modifier once - Caps Lock among them - and then expected "a". Caps Lock pressed once is on: "A" is right. The test now expects "A", presses Caps Lock again, and expects "a".

**2. A test of mine: two keys the test could not spell.** The first `gui64-test` and `aarch64-gui-test` failed: "keys typed on Dvorak did not make "echo dvorak"" - the shell had run `echo dorak` and `diplay layout u`. `gui-test.sh`'s own `keys` spells each character as itself, and "." and ";" are not QEMU key names ("dot", "semicolon"): QMP said so (`invalid parameter: .`), the PC's monitor dropped them silently. The Dvorak lines are spelled with `key-names.py` now. The repeat checks had passed in that run already.

**3. Found before it bit: the typers expected the wrong lines.** `typed_line.py` works out from the keys what each typed line says, so the typers wait for that very line to run; it assumed US. After `display layout dvorak` it would have waited ten seconds for `ran "dijs h.soav"` on every line. It now follows the layout: after a line that ran `display layout dvorak`, it works out the lines in Dvorak until one runs `display layout us`.

## How I checked it

- **Host tests**: `input` 63 (10 new, `keyboard`): the layout decides the characters (the keys of "hello" are "d.nnr" on Dvorak; "hello" is typed on j d p p s); Shift and Control follow the layout (Ctrl and Dvorak's C key is 0x03); names; a keyboard's own repeat dropped; the delay and the interval; a repeat shifted when Shift goes down meanwhile; modifiers and Caps Lock do not repeat; the newest key repeats, its up ends it; an up never seen down still ends Shift; codes past KEY_MAX refused. `kernel/abi` 38 (the LAYOUT message).
- `make coverage`: `input` 94.7% (floor 92), `kernel/abi` 87.4% (floor 87). `make unsafe-check`.
- **Boots, PC and AArch64** (`gui64-test`, `aarch64-gui-test`, both passed after bug 2), at `counter`'s window:
  - **b held for 1.2 s** (QEMU's monitor, `sendkey b 1200` - QEMU repeats nothing itself): `[counter] key b` 19 times on both, each with a "key code 48 down", and one up. 1 + 18 repeats: half a second, then 25 a second for 0.7 s. Before, the PS/2 keyboard repeated by itself and the virtio one not at all.
  - **No stray up**: `counter` is not told Enter (28) came up.
  - **Dvorak and back**: `display layout dvorak`, then the keys d i j s spc h . s o a v - the shell runs `echo dvorak`; `display layout us` typed on Dvorak's keys (h g ; r p a t ...); then `echo done` in US.
- `usb64-test` (a USB keyboard, its bytes through the display now), `edit64-test`, `deskbench64-test` (key to screen, timed from the KEY now), `screen64-test`, `aarch64-screen-test`, `boot64-test`, `aarch64-keys-test`: passed.
- `make test64` (MaK-WSL, KVM, 2026-11-28): all passed, 799 s.
- **Not tested**: a real keyboard (K1); a USB keyboard at a window (the USB test types in the terminal).

## What is still missing

- **Layouts beyond ASCII** (German, French): with K43's font. AltGr (right Alt as a third level) comes with them.
- **The repeat's delay and rate as settings** (K150, Era 23's settings); the keyboard's LEDs (Caps Lock's light).
- **The terminal as its own program** (K44): the bytes are made in the display today; when the terminal leaves it, `Keyboard` goes with whoever turns keys into bytes.

## What I learned

- **One place pays for itself once there are three.** The layout, the repeat and the bytes were each right in every driver, and each a little different; moved to the one process every key passes through, the drivers lost nothing and the AArch64 keyboard gained its repeat.
- **A test harness has its own idea of the keyboard.** `keys`, `key-names.py` and `typed_line.py` each spell keys as characters; a layout makes "the key" and "the character" different things, and each of them had to learn it.

## Testing note

```
make run64
```

| Type | You should see |
|---|---|
| Hold a letter key | It types once, then after half a second repeats steadily, about 25 a second |
| `display layout` | `usage: display layout NAME - the layouts: us dvorak` |
| `display layout dvorak`, then type on the keys labelled `dijs h.soav` | The line reads `echo dvorak`; Enter prints `dvorak` |
| On the keys labelled `hg;rpat patsfk f;`, then Enter | `display: keyboard layout us`: the keys type as labelled again |

Automated: `make gui64-test`, `make aarch64-gui-test`, `make test64`
