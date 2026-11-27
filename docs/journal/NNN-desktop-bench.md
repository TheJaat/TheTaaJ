# The desktop benchmark: what a frame costs, and how long a key takes to show

*Journal entry #NNN · a step, MaK-WSL/desktop's K114. Written on MaK-WSL/desktop. The desktop lane's first item: the numbers every item after it is measured against - one composition of the whole screen, a window dragged a pixel at a time, and the time from a key reaching the window server to its echo on the screen, while four busy programs take every CPU. Measured inside the window server, where the screen is drawn; printed by `display bench` and `display keys N`; run on 1 CPU and 4 by `deskbench64-test`.*

## Why

The desktop lane's next items rebuild the window server: `display` split into a compositor, a window manager and a terminal (K115), the terminal as its own program (K44), real key events (K116). Each of those moves work between programs, and each could make typing feel slower without any test failing. Entry 116 measured the scheduler (`schedbench`, K95) and left one of K95's numbers to this lane: **key press to screen with four busy programs**. That, and what drawing costs, are what this entry measures - before anything changes.

## What is measured, and where

```
 key ─▶ kbd ─▶ display ─▶ shell ─▶ display ─▶ composition ─▶ screen
               │ arrived            │ echo taken in  │ done: one key timed
               └────────── key to screen ────────────┘
```

| Measurement | How |
|---|---|
| **One composition** | `display bench`: the whole screen composed 21 times - the desktop, every window bottom to top, into the back buffer, then copied to the framebuffer: the most one frame can cost. Median and best |
| **Dragging** | `display bench`: the terminal dragged by its title bar a pixel at a time, 100 to the right and 100 back, every step through the same code a mouse's drag goes through (`Desktop::pointer`, the damage, the compositions): frames per second |
| **Key to screen** | `display keys 20`, then twenty letters typed at the prompt with four `forever spin` programs running: each key timed from its arrival at the display, on its way to the shell, to the end of the composition that put the shell's echo of it on the screen. Median and worst |

Why inside `display`: the composition and the drag are its own work, and nobody else can time them without the time of asking. For the key, `display` sees both ends - the key on its way to the shell, and the echo on its way to the screen - without any new message between programs. What it cannot see: the keyboard driver's part, from the interrupt to the key reaching `display` (one channel message), and the framebuffer's way to the monitor (QEMU's, not ours).

Keys are matched to their echoes by count. While keys wait, each printable byte of text from the shell is one echo; a composition after text came ends as many waits as echoes came, oldest first. At the prompt, the text that comes back while letters are typed is their echo and nothing else.

## The numbers

`deskbench64-test`, MaK-WSL under KVM, the 1024x768 screen, one boot each:

| | 1 CPU | 4 CPUs |
|---|---|---|
| one composition of the whole screen: median / best | 557 / 468 us | 492 / 453 us |
| dragging a window a pixel at a time | 200 frames in 177 ms: **1128 frames/s** | 200 frames in 180 ms: **1110 frames/s** |
| key to screen with four busy programs: median / worst | **227 / 536 us** | 161 / 303 us |

- **A frame of the whole screen costs half a millisecond**: 768 rows of 4 KiB composed into the back buffer and copied to the framebuffer. The CPUs make no difference - `display` is one thread. A screen at 60 frames a second has 16.7 ms per frame; the window server uses 3 % of it for the worst case, a full-screen change.
- **Dragging is faster than any screen shows** - over 1100 frames a second, because a drag composes only the window's old and new place, not the screen.
- **Key to screen is a quarter of a millisecond on one CPU with four busy programs.** I expected tens of milliseconds - a woken shell waiting for a busy program's slice to end - and checked before believing it: the four `forever` programs were spinning the whole time (started before `display keys 20`, killed after the twentieth key). The answer is entry 124 (K20): init gives the services on a key's way to the screen - the keyboard drivers, `display` and the shell - priority -5, "so that typing stays quick however many are running". With strict priorities, a key wakes them ahead of the busy programs at 0, every time. This is the first measurement of that decision; it holds.

## Design decisions

| Decision | Instead of | Why |
|---|---|---|
| Measured inside `display`, printed as lines | A program that asks and times | Composition is `display`'s own work; a program timing it would time the asking too |
| The drag driven inside `display`, through `Desktop::pointer` | QEMU's mouse moves | QEMU's moves come far slower than `display` can compose; through the monitor, the rate measured would be the test script's |
| Keys typed through QEMU, at the shell's prompt | Keys made up inside `display` | The key's real way: driver, `display`, the shell's process woken and scheduled among four busy ones, its echo, the composition |
| `forever spin`, four of them, killed after | `cfloat` or `cprimes` | Those end when their work is done, which under KVM is before the typing is; `forever` spins until killed |
| The test checks that each number was printed, not its size | Limits | As `schedbench` (entry 116): a busy host or an emulated CPU is slower; a benchmark that fails on a slow machine is a flaky test |

## How I checked it

- `deskbench64-test`: passed on its first run, both boots; every measurement printed, the dragged terminal back at (19, 80), the four busy programs killed.
- The key-to-screen number checked against the log, as above: the busy programs ran across the whole measurement.
- `display` and `shell` build without warnings; `make test64` (with `deskbench64-test` in it): all passed, 967 s.
- **Not tested**: AArch64 (see below); typing faster than the shell echoes, where keys and echoes are matched by count, not one by one.

## What is still missing

- **AArch64**: the same benchmark there (ramfb, the virtio keyboard through `hid`). The code is the same; the test boots only the PC.
- **The driver's part** of key to screen: from the keyboard's interrupt. It would take a time stamp carried in the key's message - which is K116's (real key events) to design.
- **Frames per second under load**: the drag is timed with nothing else running.

## What I learned

- **A surprising number is checked before it is written down** - and here it was right, and showed entry 124's decision working, measured for the first time.

## Testing note

```
make run64
```

| Type | You should see |
|---|---|
| `display bench` | `[display] one composition of the 1024x768 screen: median ... us` and `[display] dragging a window a pixel at a time: 200 frames in ... ms, ... frames/s` in the log; on the screen, the terminal slides right and back |
| `display keys 5`, then `abcde` | `[display] key to screen: timing the next 5 keys`, then `[display] key to screen: 5 keys, median ... us, worst ... us` |
| `display keys 0` | `usage: display keys COUNT (1 to 200)` |

Automated: `make deskbench64-test` (part of `make test64`)
