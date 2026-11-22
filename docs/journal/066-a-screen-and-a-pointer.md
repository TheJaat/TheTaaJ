# A screen and a pointer

*Journal entry #66 · Era 5, step A. Both machines now have a screen and something to point with. The AArch64 machine gets a screen of its own (QEMU's `ramfb`, switched on through the firmware configuration device) and a driver for virtio keyboards, mice and tablets. The PC's keyboard driver starts the PS/2 mouse that sits behind the same controller. A new crate, `gfx`, draws into the screen's memory with no machine in it, tested on the host. The display service draws the arrow, keeps it out of the console's way, and says where every click lands. That last part is what the tests read.*

```
[screen] ramfb: 1024x768, 32 bits a pixel, 3072 KiB at 0x40d5e000
[kernel] idle - the services take it from here
[init] started: registry display fs blk shell hid; supervising
[hid] virtio tablet at 0xa003800: "QEMU Virtio Tablet", 64 buffers, interrupts
[hid] virtio mouse at 0xa003a00: "QEMU Virtio Mouse", 64 buffers, interrupts
[hid] virtio keyboard at 0xa003c00: "QEMU Virtio Keyboard", 64 buffers, interrupts
[shell] ready - type a command (try help)
[display] left button at (512, 384)
[display] right button at (612, 434)
[display] middle button at (511, 191)
```

## Why

Era 5 is the desktop. Its goal is to write a journal entry in a text editor that runs on TheTaaJ, in a window. Before windows can exist, two things are needed that were missing:

- **A screen on both machines.** The PC has had one since the bootloader handed the kernel a framebuffer. The AArch64 `virt` machine had none: everything there went through the serial port.
- **A pointer.** Windows are things you click on, drag and resize. Nothing in the system knew a mouse existed.

So this step is the floor the window server will stand on. A drawing library that knows nothing about devices, a way to get pixels onto the screen on each machine, a way to get movement and clicks from each machine's pointing device, and one place (the display service) where those meet.

## The picture

```
 PC                                          AArch64 (QEMU virt)
 ──                                          ───────────────────
 PS/2 controller (ports 0x60/0x64)           virtio-mmio slots           fw_cfg
   keyboard ─┐                                 keyboard ─┐                 │
   mouse ────┤ IRQ 1 and IRQ 12,               mouse ────┤ one interrupt   │ "etc/ramfb"
             │ one vector                      tablet ───┤ each            │ (kernel, at boot)
             ▼                                           ▼                 ▼
          kbd (ring 3)                                hid (ring 3)       ramfb screen
     keys │       │ Pointer                      keys │       │ Pointer   (plain memory)
          ▼       ▼                                   ▼       ▼            │
        shell   display ◄─────────── the same POINTER message ────────────┘
                  │  draws text and the arrow with gfx
                  ▼
               framebuffer
```

Both machines end in the same place: keys go to the shell's channel as the bytes a terminal sends, and pointer reports go to the display's channel as one 11-byte message. The display neither knows nor cares which machine it is on.

## A screen for AArch64: ramfb through fw_cfg

QEMU has a device called `ramfb` that is the simplest screen there is. It shows whatever is in a stretch of the machine's own memory, as pixels, once it is told where that memory is and how the pixels are laid out. There are no registers to draw with and no commands to flush. You write a pixel to memory and it is on the screen.

It is told through **fw_cfg**, QEMU's firmware configuration device. fw_cfg is a list of named files that QEMU shares with whatever runs first in the machine. File `0x19` is the directory. The kernel reads the directory, finds the file `etc/ramfb`, and writes a 28-byte description of the screen into it:

```
  0      8      12     16     20     24    28
  ┌──────┬──────┬──────┬──────┬──────┬─────┐
  │ addr │ XR24 │flags │width │height│stride│   all big-endian
  └──────┴──────┴──────┴──────┴──────┴─────┘
```

Writing goes through fw_cfg's DMA interface. It is a 16-byte request in memory ("select file N, then write L bytes from address A"), whose physical address is written to a register. QEMU carries it out at once and writes 0 into the request's first field when done. Every number fw_cfg reads or writes is big-endian: it was made for firmware on any machine, and chose one order for all.

The bytes are in `kernel64/src/fwcfg.rs` and have no device in them, so the directory search, the request and the description are tested on the host. `main.rs` maps the registers from the device tree's `qemu,fw-cfg-mmio` node, allocates 3 MiB of contiguous memory for 1024 x 768 pixels, sends the request, and then hands the screen to init exactly as the PC's framebuffer is handed over: a `FramebufferInfo` and a memory object. From there on, the display service is the same program on both machines.

```rust
let config = fwcfg::ramfb_config(physical, width, height, stride);
let request = fwcfg::dma_write(selector, config.len() as u32, page_physical + 64);
// ... both copied into one page, then:
fence(Ordering::SeqCst);              // everything in memory before the device looks
dma(0, (page_physical >> 32) as u32); // high half
dma(4, page_physical as u32);         // low half: this write starts the request
```

The `fence` matters. The CPU may make the write to the register visible before the writes to the request page, unless told otherwise, and then QEMU would read a half-written request.

## gfx: drawing with no machine in it

`gfx` is a new top-level crate. It knows pixels and nothing else:

- `Surface` is a mutable view of pixel memory: width, height, and a *stride* (pixels from one row to the next, which can be more than the width). It can `put`, `fill` a rectangle, `copy` from an `Image`, and draw `text` in TheTaaJ's font at any scale.
- `Rect` does the clipping, and every drawing call clips. Drawing half off the screen draws the half that is on it, and drawing entirely off it does nothing. Nothing panics.
- `Pointer` is the arrow: a 12 x 19 picture, black outline, white inside.

The arrow has to be drawn on top of whatever is on the screen, and moving it must leave nothing behind. The way to do that without redrawing the whole screen is **save-under**. Before drawing the arrow, copy the pixels it is about to cover. Before moving it, put them back.

```rust
pub fn show(&mut self, surface: &mut Surface, x: i32, y: i32) {
    self.hide(surface);                       // put back where it was
    let under = Rect::new(x, y, POINTER_WIDTH as u32, POINTER_HEIGHT as u32).clip(&surface.bounds());
    for row in 0..under.height as usize {     // keep what it will cover
        let line = surface.row(under.x, under.y + row as i32, under.width);
        self.saved[row * POINTER_WIDTH..row * POINTER_WIDTH + line.len()].copy_from_slice(line);
    }
    self.saved_rect = under;
    // ... then the arrow's pixels
}
```

There is one rule for whoever else draws on the screen: hide the arrow first. If the console paints a character cell under the arrow while it is shown, the saved pixels become stale, and putting them back later would undo the character. So the display's loop hides the arrow, paints the text that changed, and shows the arrow again where the pointer now is.

A Rust note: `Surface<'a>` borrows the pixel memory (`&'a mut [u32]`). While a `Surface` exists, nothing else can write to those pixels. That is the compiler enforcing what save-under needs: in the display, `screen.surface()` borrows `screen` mutably, so the code that paints text through `screen` cannot run while the arrow is being drawn.

## The PC's mouse: the second port of the same controller

The PS/2 controller has two ports: the keyboard on the first, the mouse on the second. They share the data port (0x60) and the status port (0x64). The status byte has a bit (bit 5) that says which device the waiting byte came from. Bytes are sent to the mouse by first writing `0xD4` to the controller, which means "the next byte is for the mouse".

So the mouse belongs in the keyboard driver, `kbd`. Two drivers reading one data port would race: whichever read first would take the other's byte. On boot, kbd now enables the second port and sends the mouse `0xF6` (defaults) and `0xF4` (start reporting), each answered by `0xFA`. From then on, each byte is sorted by bit 5. A mouse packet is three bytes:

```
byte 0:  Yovf Xovf Ysign Xsign  1  Middle Right Left
byte 1:  X movement (low 8 bits; the sign is in byte 0)
byte 2:  Y movement (low 8 bits; up is positive)
```

Bit 3 of the first byte is always 1. That is how the decoder (`input::pointer::Ps2Mouse`) finds the start of a packet again if a byte is lost: a "first byte" without bit 3 cannot be one. PS/2 counts Y *up*, while the screen counts down, so the decoder turns Y over.

The mouse's interrupt is ISA IRQ 12. The kernel routes it to the same vector as the keyboard's IRQ 1, so the one interrupt handle kbd already waits on wakes it for both. kbd then drains every byte the controller holds, whichever device each came from.

## hid: virtio keyboards, mice and tablets

On AArch64 the keyboard and pointer are **virtio-input** devices: the same virtio-mmio transport the disk and network drivers already use, with device id 18. A virtio-input device sends events into its queue, each 8 bytes in Linux's evdev format: a type, a code and a value. `EV_KEY 30 1` is "key A pressed"; `EV_REL REL_X -3` is "moved 3 to the left"; `EV_SYN` ends a group of events that belong together.

`hid` is a new driver process. init hands it every virtio input device the kernel found (registers, DMA memory, interrupt). For each one it asks the device's configuration space what the device is: its name, which event types and codes it can send, and for a tablet the range of its axes. That decides its kind:

```rust
pub fn kind(keys: &[u8], relative: &[u8], absolute: &[u8]) -> Kind {
    if has_bit(keys, KEY_A) { Kind::Keyboard }
    else if has_bit(absolute, 0) && has_bit(absolute, 1) { Kind::Tablet }
    else if has_bit(relative, 0) && has_bit(relative, 1) { Kind::Mouse }
    else { Kind::Other }
}
```

Then it fills the queue with 64 empty buffers and waits on all the devices' interrupts at once.

**Keys** go a way that reuses everything the PC already has. Linux numbered its key codes after scancode set 1: codes 1 to 88 *are* the set-1 scancodes (Escape is 1 in both, F12 is 88). The keys the original PC keyboard did not have (the arrows, Home, Delete) got new codes above 88 in Linux, while set 1 gives them a `0xE0` prefix. So `input::evdev::scancode` turns a virtio key into the set-1 bytes for the same key, and they are fed to the same `Decoder` the PS/2 driver uses. There is one place in the system that knows Shift-2 is `@`.

**Pointer events** are collected until `EV_SYN` and then sent as one report. A mouse sends relative moves. A tablet sends absolute positions, which is what QEMU's window uses so that the arrow follows the host's own pointer without grabbing it.

## Where the pointer is: the display decides

Both kinds of device move one pointer, and only the display knows how big the screen is. So drivers do not track a position. They pass on what the device said, in one message:

```
POINTER (4) │ absolute (u8) │ x (i32) │ y (i32) │ buttons (u8)      11 bytes
```

Relative: x and y are a move, right and down positive. Absolute: x and y are 0 to 32767 across the screen, a range independent of any screen size. The display clamps relative moves to the screen, and scales absolute positions so that 0 is the first pixel and 32767 the last:

```rust
self.x = (i64::from(report.x) * i64::from(right) / i64::from(ABSOLUTE_MAX)) as i32;  // right = width - 1
```

Then, for each button that went down, it logs one line:

```
[display] left button at (783, 527)
```

That line is what a click will mean once there are windows. For now it is what the tests check positions by.

## Testing a pointer without a hand on it

`make screen64-test` and `make aarch64-screen-test` boot each machine with a screen, type a line, move the pointer to known places, click at each, and check:

- the `[display] … button at (x, y)` lines, in order, against the positions worked out by hand;
- the screenshot (QEMU's `screendump`): the console is painted as before (`check-screen.py`), and a new `check-pointer.py` finds the arrow's 114 visible pixels exactly where the last click was, with no white pixel anywhere else (an arrow left behind would show up there).

On the PC the moves are the monitor's `mouse_move`, which feeds the PS/2 mouse. On AArch64 they go through QMP, QEMU's JSON control socket (`qmp-type.py`), because the monitor's `mouse_move` moves only relative pointers and the tablet is absolute. The AArch64 run has a mouse *and* a tablet in one boot. It moves by steps and jumps to places, both moving the one pointer:

| Step | Device | Expected click |
|---|---|---|
| start | | left at (512, 384), the middle |
| move 100, 50 | mouse | right at (612, 434) |
| jump to 16384, 8192 | tablet | middle at (511, 191) |
| jump to 0, 0 | tablet | right at (0, 0) |
| jump to 32767, 32767 | tablet | left at (1023, 767) |
| move −240, −240 | mouse | left at (783, 527), and the arrow is drawn there |

The PC run goes past the corner with six moves of 120 and checks that the pointer stops at (1023, 767).

## Design decisions

| Decision | Chosen | Rejected, and why |
|---|---|---|
| A screen for AArch64 | `ramfb`, described once through fw_cfg | virtio-gpu: every update is a command (create a resource, attach memory, transfer, flush); more protocol for the same pixels. `bochs-display`: a PCI device, and the AArch64 kernel has no PCI |
| Input on AArch64 | virtio-input over virtio-mmio | QEMU's PCI virtio-input (modern only, and no PCI here). A USB tablet: needs a USB host controller driver and the USB stack first |
| Where the PC's mouse is driven | In `kbd`, with the keyboard | A separate mouse driver: two processes reading one data port, and each byte goes to whichever reads first |
| IRQ 12 | Routed to the keyboard's vector | A vector of its own: kbd would wait on two handles for one controller, and still read the same status byte to sort the bytes |
| Virtio keys | Turned into set-1 bytes, decoded by the PS/2 decoder | A second decoder from Linux codes: two places to keep Shift, Caps Lock and the layout right |
| Who knows where the pointer is | The display | The drivers: they do not know the screen's size, and a mouse and a tablet would each keep a position of their own |
| Absolute positions | 0..32767 in the message, scaled by the display | Pixels, scaled by the driver: the driver would need the screen size, and would have to be told again when it changes |
| Drawing the arrow | Save-under, hidden around every text update | Redrawing everything under it on each move: the console does not keep pixels, only cells. A hardware cursor: ramfb has none, and the PC's framebuffer neither |
| The message | One `POINTER` message on the display's existing channel | A channel of its own: one more handle to pass around for no gain; the display reads one queue in order |
| `make run-aarch64` | Unchanged (serial terminal); a new `make run-aarch64-screen` opens a window | Switching the default to the window: the serial terminal is what I use most on AArch64, and what the scripts expect |
| Testing the tablet | QMP `input-send-event`, with the monitor's `mouse_set` choosing which device gets buttons | The monitor alone: it cannot move a tablet |

## Bugs

- **`gfx::copy` landed in the wrong place when the source was cut.** It clipped the source rectangle to the source image *first* and worked out the offset from the clipped rectangle. With a rectangle hanging off the source's top-left, everything shifted by the part that was cut. The fix is to compute how far everything moves from the rectangle as asked, then clip. The host test `copying_lands_where_asked_and_is_cut_at_both_ends` found it.
- **The PC lost the first key, and the mouse did not start, when keys arrived during start-up.** Typing right after boot gave `cho hi` instead of `echo hi`, and no mouse. The controller has one byte to give at a time from either device. A key pressed while kbd was waiting for the mouse's `0xFA` sat *in front* of the answer. kbd read it, saw it was not `0xFA`, decided there was no mouse, and threw the key away. Now `tell_mouse` reads every byte, keeps the keyboard's in a small `Early` buffer (decoded once the main loop runs), and takes only a byte with the mouse bit as the answer. Three boots in a row with keys typed during start-up then gave `echo hi` and a working mouse.
- **A ready line I almost renamed.** I first changed kbd's line to say "keyboard and mouse ready". `qemu-boot.sh` and `check-boot.sh` wait for `[kbd] PS/2 keyboard ready` before typing, so every PC boot test would have typed nothing. The line keeps its start and gains `; the mouse too` at the end.
- **QEMU's PS/2 mouse keeps what it cannot send.** A packet can carry −256 to 255, and QEMU sends at most ±127. A `mouse_move 5000 0` is not cut off: the rest stays pending in QEMU and comes out in later packets, and it swallowed a later move of −300 in my first tries. That is QEMU behaving as a mouse would, not a bug of mine. The tests move by 120 at a time.
- **QEMU aborted when told which device to send to.** QMP's `input-send-event` has a `device` argument, and I used it to pick the mouse or the tablet. QEMU 8.2 died with `Property 'qemu-fixed-text-console.device' not found`. With `-nodefaults` it refuses instead: `Device mouse (head 0) is not bound to a QemuConsole`. That argument names a display head the device is bound to, which a virtio mouse never is. Without it, QEMU sends a relative move to the one device that takes those (the mouse), an absolute one to the tablet, and a button to whichever is current. The monitor's `info mice` and `mouse_set` choose that, so the test says `@use,Mouse` or `@use,Tablet` before its clicks. It matters: a tablet's button report carries the tablet's own position, and the pointer would jump there.
- **The ramfb request page was freed before the code knew QEMU was done with it.** `drop(page)` came before the check of whether the request had finished. QEMU always finishes at once, so no test could see it. But a device that had not finished would write into memory already given back. Now the page is kept (leaked) if the request never finishes.
- **Two mistakes in my test tooling.**
  - `check-pointer.py` first loaded `check-screen.py` as a module to reuse its PPM reader. That script runs its checks when loaded, so it exited with *its* verdict, taking the arrow's x for its ink limit. `check-pointer.py` now has its own reader.
  - My first round of mutation experiments reported every mutant caught. The shell I ran them from did not have cargo on its PATH (the Makefile adds it itself), so every run failed before compiling, and "failed" looked like "caught". The same mistake made a clippy run look clean when it had not run. I redid both with cargo found and with the script telling apart a failing test, a passing one, and one that never ran. The real clippy run then found five warnings in a comment I had written.
  - Putting the file back after a mutant left a mutant in the build. The script restored each file by moving its saved copy back, and a moved file keeps its old modification time. The build made from the mutant was newer than that, so cargo, which decides what to rebuild by those times, kept the mutant. The next `make test64` failed in `a_file_is_found_in_the_directory_by_its_whole_name`, with a test binary still matching names by prefix, though the source was right. I touched every file I had mutated and ran it all again. A restore must give the file a new time (copy it back, or `touch` it), never the old one.

## How I checked it

Automated, on the host:

- `gfx`: 8 tests. Clipping in every direction, `fill` and `copy` cut at both ends, text, the arrow's picture, and that showing, moving and hiding the pointer (moved, half off the bottom-right, half off the top-left) leaves exactly the pixels that were there.
- `input`: 41 tests. New: PS/2 packets (signs, overflow, buttons, Y turned over), finding the start of a packet again after a lost byte, evdev events into reports for mice and tablets, and virtio key codes through the PS/2 decoder (letters, Shift, the 0xE0 keys, Right Control).
- `virtio`: 12 tests. One new: a virtio-input event read, the configuration's bitmaps and axis ranges, and what kind a device is from what it sends.
- `kernel/abi`: 23 tests. New: a `POINTER` message there and back, and absolute positions outside 0..32767 refused.
- `kernel64`: 3 new tests for fw_cfg. The directory search (by whole name, past the count, a full 56-byte name, cut short), the DMA request's bytes, and the ramfb description's bytes.

Automated, by booting: `make screen64-test` and `make aarch64-screen-test` (the table above), added to `make test64`. The existing AArch64 tests boot without ramfb, so they still get the serial terminal, and they still pass.

Mutation experiments, host: 9 mutants, all caught, each by a named test:
- `copy` clipping before the offset (the bug above);
- `hide` not putting the pixels back;
- PS/2 Y not turned over;
- no resync on bit 3;
- the Up key given Down's code;
- an axis range read big-endian;
- the ABI accepting positions past 32767;
- the fw_cfg request without SELECT;
- the directory matching a name by its prefix.

Mutation experiments, by booting: 3 mutants, all caught by the screen tests.
- The display not clamping x: clicks at (1332, 767) and (1092, 527). The arrow drawn off the screen did not crash anything, because `gfx` clips.
- Tablet positions scaled by the width instead of the last pixel: the far corner became (1024, 767), a pixel off the screen.
- kbd marking its reports absolute: every click at (0, 0), because small moves read as positions near the corner and negative ones are refused by the ABI.

By hand:
- on the PC, clicks after moves through the monitor, and the arrow's pixels in a screenshot (where I first saw `check-pointer.py`'s bug);
- on AArch64, `make run-aarch64-screen` started without a window (`RUN_A64_DISPLAY="-display none"`): ramfb, the keyboard and the tablet found, and the shell started on the screen.

Not tested:
- QEMU's window with a real hand on the mouse, on either machine. Everything above drives the pointer through QEMU's monitor and QMP, and I have not watched the arrow follow a real mouse.
- Real hardware.
- A PS/2 mouse that answers something other than `0xFA`: it would be treated as absent.
- A virtio keyboard's LEDs (Caps Lock's light).
- A scroll wheel: its events are read and ignored.
- More than four input devices: the rest are not handed to hid.

## What is still missing

- Windows. The pointer has nowhere to click yet; that is step B: a window server with surfaces in shared memory, focus, and dragging.
- The scroll wheel, and buttons past the third.
- Mouse speed and acceleration: one count of the device is one pixel.
- Screen sizes other than 1024 x 768 on AArch64 (the kernel picks the size; ramfb would take others).
- USB. Real PCs today have no PS/2 ports, and real ARM boards no virtio. TheTaaJ's pointer works only where QEMU provides these devices.
- The arrow can flicker. It is hidden while text is painted, so a busy console shows it off and on.

## What I learned

- The simplest screen is a piece of memory with a description. ramfb has no commands at all; the work is in telling QEMU where the memory is, through a protocol made for firmware.
- fw_cfg is big-endian on every machine, and a `fence` is needed before telling a device to read what the CPU just wrote.
- The PS/2 controller has one byte to give at a time, from two devices. Every byte must be read, and sorted by the status bit. Assuming the next byte is the answer you are waiting for loses keys.
- Linux key codes are scancode set 1, renumbered only for the keys the 1981 keyboard did not have. A whole translation layer became a 15-line match.
- Save-under is a contract. Whoever draws under the arrow must hide it first, and the borrow checker can enforce the "not at the same time" part.
- The thing that knows the screen's size should own the pointer's position. Drivers pass on what the device said; relative and absolute devices then move the same pointer without knowing about each other.
- A test harness must tell "failed" from "never ran". My mutation script did not, and for one round it reported success because nothing ran at all.
- Emulators have behaviours of their own, and tests have to respect them: QEMU's pending PS/2 deltas, and QMP's `device` argument meaning something other than it seems.

## Testing note

**1. The PC: the arrow and clicks**

```
make run64
```

Click inside the QEMU window: QEMU then sends the host's mouse to the machine as a PS/2 mouse. **Ctrl+Alt+G** gives the mouse back to the host.

| Type / do | You should see |
|---|---|
| (at boot, in the terminal) | `[irq] PS/2 mouse on GSI 12, sharing the keyboard's vector` and `[kbd] PS/2 keyboard ready (driver in ring 3, process N); the mouse too` |
| move the mouse | a white arrow with a black outline moves on the screen |
| click left, right, middle | in the terminal: `[display] left button at (X, Y)`, `right …`, `middle …`, with where the arrow's tip was |
| move far past any edge | the arrow stops at the edge; a click there says `(0, …)`, `(1023, …)`, `(…, 0)` or `(…, 767)` |
| `echo hi` | `hi` on the screen; where the text is drawn under the arrow, the arrow stays on top and nothing is left behind when it moves away |

(I drove these with the monitor's `mouse_move` and `mouse_button`, not a hand on a mouse: these rows are what a real mouse should give.)

**2. AArch64: a screen, a keyboard and a tablet**

```
make run-aarch64-screen
```

QEMU opens a window; the kernel's log stays in the terminal. Close the window to leave.

| Type / do | You should see |
|---|---|
| (at boot, in the terminal) | `[screen] ramfb: 1024x768, 32 bits a pixel, 3072 KiB at …`, `[hid] virtio tablet at …: "QEMU Virtio Tablet"`, `[hid] virtio keyboard at …: "QEMU Virtio Keyboard"` |
| (in the window) | the amber band, the gradient, and the shell's prompt, as on the PC |
| `echo hi` (typed in the window) | `hi`, on the screen |
| move the host's mouse over the window | the arrow follows it with nothing to grab: the tablet sends where the host's pointer is |
| click near the window's top-left, then bottom-right corner | `[display] left button at (X, Y)` close to `(0, 0)`, then close to `(1023, 767)` |

`make run-aarch64` is unchanged: the serial terminal, no window.

(I started this without a window, `RUN_A64_DISPLAY="-display none"`, and saw the boot lines and the shell; the window rows are what it should show.)

**3. The tests by themselves**

```
make screen64-test
make aarch64-screen-test
```

| Look for | Meaning |
|---|---|
| `clicks: left (512, 384);right (612, 434);middle (1023, 767);left (783, 527)` (PC) | each click at the place worked out by hand, and the pointer stopped at the corner |
| `clicks: left (512, 384);right (612, 434);middle (511, 191);right (0, 0);left (1023, 767);left (783, 527)` (AArch64) | the mouse's steps and the tablet's jumps both moved the one pointer |
| `check-pointer: the arrow is at (783, 527), 114 pixels of it on the screen, and nowhere else` | the screenshot has the arrow where the pointer is, and none left behind |
| `screen64-test passed` / `aarch64-screen-test passed` | |

The screenshots are `build/screen64.log.ppm` and `build/aarch64/screen.log.ppm`, and they open in an image viewer.

Host checks:

```
cd gfx && cargo test           # 8 tests: clipping, copy, text, the arrow, save-under
cd input && cargo test         # 41 tests: keys, PS/2 packets, evdev events, virtio keys
cd virtio && cargo test        # 12 tests, the input device's configuration among them
cd kernel/abi && cargo test    # 23 tests, the POINTER message among them
```

Automated: `make test64`
