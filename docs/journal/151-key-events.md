# Keys, not only bytes: a window is told when Shift goes down

*Journal entry #151 · a step, MaK-WSL/desktop's K42. Written on MaK-WSL/desktop. A window used to hear about the keyboard only in a terminal's bytes - "a", "\x1b[A" for Up - so a key that makes no byte never reached it: Shift alone, Control alone, any key going up. Now every key, down and up, reaches the window with the focus as a key event: which key (Linux's key codes), down or up, and the modifiers held. The terminal still gets its bytes, unchanged.*

## Why

A game moves while a key is held and stops when it comes up; a drawing program constrains a line while Shift is down; push-to-talk (Era 11) talks while a key is held. None of that can be built on bytes: a byte is made when a key goes down, and nothing is made when it comes up. K42 is the smallest piece of K116 ("real key events through the window protocol") that makes that possible - and the first half of its design: key codes, down and up, modifiers. Repeat and a layout chosen in user space stay K116's.

## How it works

```
 PS/2 ─ kbd ─┐                      ┌─ bytes ─▶ the shell (focus: the terminal)
 virtio ─ hid ┼─▶ Decoder ─ event ─┤          or Event::Key (a program's window)
 USB ─ usb ──┘   (+ its code)       └─ KEY {code, down} ─▶ display
                                          modifiers kept ─▶ Event::KeyCode (a program's window)
```

- **Every keyboard already reaches one decoder** (`input::Decoder`) as PC scancodes: PS/2 sends them, and the virtio and USB drivers turn their keys into them first (`evdev::scancode`), so that one place knows the layout and what Shift-2 is. The decoder now also remembers **which key** it just decoded, as Linux's key code (`Decoder::code`) - the reverse of `evdev::scancode`'s table: codes 1 to 88 *are* the scancodes, and the keys the first PC keyboard did not have (the arrows, Home, Delete) are the 0xE0-prefixed ones.
- **Each driver sends `abi::display::KEY`** - the code and 1 or 0 for down or up - to the display, on the copy of the display's channel it already has for the mouse. Besides the bytes, not instead. A key's repeat (USB's own, virtio's value 2) is not a new press: repeat is K116's.
- **The display keeps the modifiers held** (Shift, Control, Alt, left or right) and sends `Event::KeyCode { code, down, modifiers }` to a program's window with the focus. The terminal is not told: what it needs, the bytes, comes as before.

Why key codes rather than characters: a code names a key, not what the layout makes of it. 30 is the A key whether it types "a", "A", or (on another layout) "q". A game wants the key; a text field wants the character - and still has the bytes for that.

## Design decisions

| Decision | Instead of | Why |
|---|---|---|
| Linux's key codes | Our own numbering; set-1 scancodes | Linux's numbering is the one virtio and evdev already use, and USB's maps to it (`input::usb::key_code`); scancodes need a prefix byte and are a PC's |
| Events besides the bytes | Events only, the terminal turning them into bytes | That is K116 - a layout in user space, the terminal its own translator. K42 adds, and changes nothing that works |
| Modifiers kept by the display | Kept by each driver | One display, several keyboards: Shift on one and A on another is still Shift-A |
| On the drivers' existing channel to the display | A new channel | Each driver already has one for the mouse; init needs no change |
| Repeats not sent as presses | Sent as more downs | A held key is one press until it comes up; how a program wants repeats is K116's question |

## Bugs

**1. `key_message` had no test** - the night run (2026-11-27/28, TheJat-WSL) failed `k42-key-events` on one thing only: the coverage floor (J96, entry 137), `kernel/abi` at 86.6% of its lines against a floor of 87%. The new `display::key_message` was used by three drivers and read by the display, and checked in the boots, but no host test called it, so its lines counted as untested. Fixed with `a_key_message_is_its_code_and_whether_down`: the bytes spelled out (Shift down, Control up, a code past 255 with its high byte second), and for several codes the message read back the way `display.rs` reads it. `make coverage` after: `kernel/abi` 87.2%, every crate at or above its floor. The lesson: `make coverage` belongs among a branch's own checks before the night queue, beside the host tests.

Otherwise none found by the tests. One behaviour worth writing down, seen in the first test log: a window can be told a key **came up** that it never saw go down. The test types Enter to start `counter`; Enter goes down while the terminal has the focus, `counter`'s window opens and takes it, and `counter` is told "key code 28 up". It is true - the key did come up - and harmless to a program that tracks keys it saw go down. Whether the display should hold such an up back is K116's to decide.

## How I checked it

- **Host tests**: `input` 49 (two new: every key code comes back from its scancode, both tables; the decoder names the key it decoded, Shift and an 0xE0 key included), `gui` 38 (the new event there and back, with modifiers; cut short, and with a "down" neither 0 nor 1, refused), `abi` 37 (one added after the night run: bug 1).
- **Boots**: `gui64-test` and `aarch64-gui-test` - Shift alone and Control alone pressed at `counter`'s window, on the PC's PS/2 keyboard (kbd) and on AArch64's virtio keyboard (hid). Both logs show the same events: `+` as Shift down, = down, = up, Shift up, with the modifiers right at each; `a` down and up; Shift alone (42, modifiers 1, then 0); Control alone (29, modifiers 2, then 0). `usb64-test` (a line typed on a USB keyboard, through `usb`'s changed path) and `edit64-test` (the editor, which ignores the new event).
- The boot tests ran while another tree's `make test64` held the test lock - small boots, each throttled by `boot-slots.sh` - and passed the first time.
- `make test64`: the night run of 2026-11-27/28 passed everything but the coverage floor (bug 1); with its fix, `make coverage` passes, and the branch goes to the next night's queue.
- **Not tested**: a USB keyboard at a program's window (the USB test has no window); a real keyboard (K1).

## What is still missing

- **K116**: repeat as events, the layout in user space, the terminal turning events into bytes - and whether an up never seen down is held back.
- A real keyboard, on the real PC.

## What I learned

- **One decoder for every keyboard paid off again**: three drivers, and the key's code came from one function in one place.

## Testing note

```
make run64
```

| Type | You should see |
|---|---|
| `counter &` | counter's window, with the focus |
| Press and release Shift alone | In the log: `[counter] key code 42 down (modifiers 1)`, then `[counter] key code 42 up (modifiers 0)` |
| Press and release Control alone | `[counter] key code 29 down (modifiers 2)`, then `up (modifiers 0)` |
| Type `a` | `[counter] key a` (the byte, as before), and `key code 30 down` and `up` |

Automated: `make gui64-test`, `make aarch64-gui-test` (both part of `make test64`)
