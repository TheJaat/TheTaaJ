# The scroll wheel, and two more buttons

*Journal entry #NNN · a step, MaK-WSL/desktop's K46 (part: the wheel and the buttons; acceleration left). Written on MaK-WSL/desktop. Mice have had a wheel for thirty years; TheTaaJ read three bytes per movement and dropped everything else. Now the PS/2 mouse is asked for its wheel - the IntelliMouse "knocks" - and the virtio and USB mice's wheels and side buttons are read too. A turn of the wheel reaches the window under the pointer as `Event::Wheel`; the side buttons travel with the others.*

## Why

Scrolling - a long file in `edit`, a list, the terminal's history one day - wants the wheel, and a browser's back and forward want the side buttons. K46 also lists pointer acceleration; that part is left, for a reason below.

## How it works

### A PS/2 mouse has to be asked

A PS/2 mouse starts as the 1980s one: three bytes per movement, three buttons. A mouse with a wheel switches to four-byte packets only when it hears a "knock" - three sample-rate settings in a row that no driver would set for their own sake - and then says so in its ID:

```
 set rate 200, 100, 80   then "who are you?"   3: IntelliMouse - a 4th byte, the wheel
 set rate 200, 200, 80   then "who are you?"   4: Explorer - 4 bits of wheel, buttons 4 and 5
```

`kbd` knocks after resetting the mouse; a mouse that knows neither knock ignores the rates and still answers 0, and its packets stay three bytes. `input::pointer::Ps2Format` is what the answer means, and `Ps2Mouse::with_format` reads packets of that length. QEMU's PS/2 mouse answers 4: the boot log says `the mouse too, with a wheel and five buttons`.

**The wheel's sign**: PS/2 sends a turn towards the user as positive. Linux turns it round - positive is away from the user, scrolling up - and so does this decoder; the virtio and USB mice already count that way.

### virtio and USB

A virtio mouse or tablet sends the wheel as `REL_WHEEL` events and the side buttons as `BTN_SIDE` and `BTN_EXTRA`, which `input::pointer::Events` used to drop; it adds them up between two SYNs now, like the movement. A USB boot-protocol mouse that sends a fourth byte has its wheel there, and its side buttons in bits 3 and 4 of the first.

### To the window under the pointer

The pointer's message to the display (`abi::display::Pointer`) has a wheel byte now, and the buttons two more bits (`SIDE`, `EXTRA`). The display sends `Event::Wheel { x, y, clicks }` to the program whose window's content is **under the pointer** - not the one with the focus: a window behind can be scrolled without clicking it first, as every desktop does. The side buttons reach a window in `Event::Pointer`'s buttons, like the others, and the display logs them as it logs the rest.

## Design decisions

| Decision | Instead of | Why |
|---|---|---|
| Ask for ID 4, the Explorer format | Stop at ID 3 | The side buttons come with it; a mouse that only knows 3 still gets its wheel |
| Linux's sign for the wheel | The wire's | One sign for three kinds of mouse, and the one programs ported later will expect |
| The wheel to the window under the pointer | To the focus | What every desktop does; scrolling should not need a click |
| **No acceleration yet** | Doubling fast moves (the classic two-step) | The PC's GUI test moves the PS/2 mouse by exact amounts through QEMU and checks the pointer lands on exact pixels; with acceleration every position after a fast move is off. Acceleration needs that test on an absolute tablet first - a change of its own |

## Bugs

**1. Mine: QEMU's wheel sign.** The PC's test turned the wheel with QEMU's monitor, `mouse_move 0 0 -1`, meaning up; counter was told -1, AArch64's `wheel-up` +1. QEMU's monitor takes a *positive* dz as up (`dz > 0 ? WHEEL_UP : WHEEL_DOWN`); its PS/2 mouse sends up as -1 on the wire, which the decoder turns into +1. The decoder was right - and the AArch64 path, arriving by another way entirely, agreed with it. The test says `mouse_move 0 0 1` now, with the reason beside it.

**2. An old test that said the wheel was ignored.** `a_virtio_mouse_reports_at_each_sync` checked that code 8 - the wheel - changes nothing. It changes something now; the check uses the horizontal wheel (code 6), which still does not.

**3. Mine: acceleration added, then taken out** before it was ever used: see the design decisions. No unused code is left of it.

## How I checked it

- **Host tests**: `input` 53 (new: an Explorer mouse's wheel and side buttons, a wheel mouse's whole-byte wheel, an ID naming its format and an unknown one taken as plain, a virtio mouse's and tablet's wheel and side buttons, a USB boot mouse's fourth byte), `gui` 38 (the wheel event there and back; cut short, refused), `abi` 36 (the pointer message with its wheel, and one a byte short or long refused).
- **Boots**: `gui64-test` - the PS/2 mouse answers the knocks with 4 and the wheel turned up once over counter reaches it as `wheel 1 at (64, 74)`; `aarch64-gui-test` - the tablet's `wheel-up`, the same line; `boot64-test` (the whole typed session, through the keyboard the mouse's knocks share a controller with), `usb64-test`, `edit64-test`: all passed. The boots ran beside the race hunt, unlocked (small, throttled by `boot-slots.sh`).
- `make test64`: RESULT.
- **Not tested**: the side buttons in a boot (QEMU's monitor has no way to press them on the PC); a real mouse (K1).

## What is still missing

- **Acceleration**, after the PC's GUI test moves to an absolute tablet.
- **The horizontal wheel** (tilting it): `REL_HWHEEL`, and PS/2's Explorer packets have no room for it.
- Programs that scroll: `edit` ignores the wheel for now (J49 is the editor's next step).

## What I learned

- **A sign can be checked from two sides.** The PC and AArch64 reach counter by different devices and different drivers; when they disagreed, the one that matched Linux's convention, on its own path, settled which was the test's mistake.

## Testing note

```
make run64
```

| Do | You should see |
|---|---|
| Watch the log as it boots | `[kbd] PS/2 keyboard ready ...; the mouse too, with a wheel and five buttons` |
| `counter &`, then turn the mouse's wheel over its window | `[counter] wheel 1 at (...)` for each click up, `wheel -1` down |
| Turn it over the terminal | Nothing: the terminal has nothing to scroll yet |

Automated: `make gui64-test`, `make aarch64-gui-test` (both part of `make test64`)
