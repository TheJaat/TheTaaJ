# USB: an xHCI driver, with keyboards and mice

*Journal entry #88 · Era 7, step R4. TheTaaJ now drives a USB 3 host controller (xHCI), and the keyboards and mice plugged into it. Like the disk and network drivers, it is one ring-3 program, `usb`. Everything it does with the controller is in a crate, `usb/`, tested against a simulated controller with simulated devices. A USB keyboard types into the same terminal as the PS/2 one, through the same decoder. A key held down repeats. A USB mouse moves the same pointer. Devices can be plugged in and pulled out while the machine runs.*

## Why

A PC without PS/2 ports is now the normal PC. The keyboard and mouse on my desk are USB, and so is every port on the front of the case. Without USB, TheTaaJ on real hardware (R7) has no keyboard at all, unless the firmware's "legacy USB" emulation pretends to be a PS/2 controller. That emulation stops the moment an operating system takes the controller over, and it does not exist at all on many UEFI machines. USB is also the road to R6: booting from a USB stick needs the same controller driver, plus mass storage on top.

xHCI is the one controller to drive. It replaced UHCI, OHCI and EHCI (USB 1 and 2) and handles every speed itself, from a 1.5 Mbit/s keyboard to a 5 Gbit/s disk. Every PC since about 2012 has one.

## What it is

```
 kernel: devices.rs            init                          usb (user/src/bin)                      shell / display
 ------------------            ----                          ------------------                      ---------------
 class 0C.03.30 (xHCI)   ─►    a "usb" service, if a     ─►  usb crate: Controller::start,       ─►  keys: the PS/2 decoder,
 BAR 0, 276 KiB of DMA,        controller was found;          device::attach per port;                 terminal bytes (3)
 MSI, else MSI-X, else         pings; stopped after hid       input::usb: reports to keys,          ─►  pointer reports (37)
 polled                                                       key repeat, mouse moves
```

- **`usb/` crate (TheTaaJ, beside `e1000/`):**
  - `xhci`: the controller's registers; TRBs and rings; `Controller<B: Bus>` with `start`, the commands (Enable Slot, Address Device, Configure Endpoint, Evaluate Context, Reset Endpoint, Set TR Dequeue, Disable Slot, No-op), control transfers, interrupt IN transfers, and events;
  - `descriptor`: the standard requests, and the device and configuration descriptors read carefully (lengths checked, nothing trusted);
  - `device`: `attach(controller, port)`, which enumerates whatever is on a port and sets up its boot keyboard or boot mouse interfaces.

  14 host tests against `Sim`, a simulated xHCI controller with simulated keyboards, mice and other devices behind its ports.
- **`input::usb`** (the `input` crate, beside the PS/2 decoder):
  - `BootKeyboard`: an 8-byte boot report in, key presses and releases out, as Linux key codes;
  - `Repeat`: when a held key repeats;
  - `boot_mouse`: a 3-byte boot report in, a relative pointer move out.

  6 host tests.
- **`usb`** (program): maps BAR 0 and the DMA memory, starts the controller, looks at every port once, then handles events. A key goes through `input::evdev::scancode` (key code to PS/2 set-1 bytes), then the same `Decoder` and `terminal_bytes` that `kbd` and `hid` use. Layout, Shift and Ctrl therefore behave exactly as they do on PS/2.
- **Kernel:** `Kind::Xhci` in `devices.rs`, recognised by its class code (0C.03.30), with 69 pages (276 KiB) of DMA memory.
- **init:** a `usb` service when the kernel handed it a controller. It gets the controller's handles, the keys' channel (as `kbd` and `hid` have) and the display's pointer channel. It is pinged like `hid`, and stopped right after `hid` during `poweroff`.
- **Makefile:** `make run64 RUN_USB=yes` adds a controller, a keyboard and a mouse; `make usb64-test` (in `make test64`).

## How an xHCI controller is driven

An xHCI controller is talked to almost entirely through memory, not registers. The registers say where things are and ring doorbells. The work is in **rings** of 16-byte **TRBs** (transfer request blocks) in DMA memory:

- the **command ring**: the driver writes commands ("give me a slot for a new device"), then rings doorbell 0;
- the **event ring**: the controller writes events ("command done", "transfer done", "a port changed"), and the driver reads them;
- a **transfer ring** per endpoint of each device: the driver writes "read up to 8 bytes into this buffer", then rings that device's doorbell.

**Who owns a TRB** is decided by one bit in it, the **cycle bit**, compared with a cycle state each side keeps. The producer writes TRBs with its current cycle value. The consumer takes TRBs while their bit matches what it expects. At the end of a ring a **Link TRB** points back to the start, with a "toggle" flag: following it flips the cycle state. Last time round's TRBs, with the old bit, are therefore never mistaken for new ones. There are no head and tail registers to keep in step, as on the e1000.

**Contexts** describe each device to the controller: a *slot context* (its port, speed, how many endpoints) and an *endpoint context* per endpoint (its type, packet size, interval, and where its transfer ring is). The driver fills an *input context*, and commands such as Address Device and Configure Endpoint copy it into the controller's *output context*. The **device context base address array** (DCBAA) maps slot numbers to output contexts. Entry 0 points at the **scratchpad** pages, memory some controllers ask the operating system to lend them. QEMU's asks for none; the simulator asks for four.

Starting the controller (`Controller::start`):
1. Take it from the firmware: the "USB Legacy Support" extended capability has a BIOS-owned bit and an OS-owned bit. Set the OS bit, and wait for the BIOS to drop its bit. Then switch off the firmware's SMIs.
2. Stop it, reset it, and wait until it is ready again.
3. Give it the DCBAA (with scratchpads), the command ring, and one event ring through interrupter 0. Interrupts are limited to one every 250 µs.
4. Run.

**Enumerating a device** (`device::attach`):
1. Reset the port; a USB 2 port then enables and says the device's speed. A USB 3 port enables itself.
2. Enable Slot gives the device a slot number.
3. Address Device: endpoint 0's ring and a first guess at its packet size (8 bytes at full speed, 64 at high, 512 at SuperSpeed). The device now has an address.
4. GET_DESCRIPTOR(device), 8 bytes first: the real packet size of endpoint 0. If it differs from the guess, Evaluate Context fixes it. Then all 18 bytes: vendor, product, class.
5. GET_DESCRIPTOR(configuration), 9 bytes, then its whole length: interfaces and endpoints.
6. SET_CONFIGURATION. Then Configure Endpoint, for the interrupt IN endpoint of each boot keyboard (interface class 3, subclass 1, protocol 1) or boot mouse (protocol 2).
7. For each of those: SET_PROTOCOL(boot), then SET_IDLE (send a report only when something changes). Then queue the first read.

A device with neither interface is named in the log and let go: its slot is disabled.

## Keyboards: reports, not scancodes

A PS/2 keyboard sends a code when a key goes down and another when it comes up. A USB keyboard in the boot protocol sends a **report**, an 8-byte picture of what is held *now*:
- byte 0: the eight modifiers as bits (left and right Ctrl, Shift, Alt, GUI);
- byte 1: reserved;
- bytes 2-7: up to six other keys held, as HID usage codes.

So `BootKeyboard` keeps the last report and compares: a key in the new report and not the old one went down, and the opposite came up. All six slots set to 1 means "too many keys" (ErrorRollOver). That report is ignored, since it says nothing about which keys are held.

The keyboard does not repeat a held key; the host does. `Repeat` starts the clock when a key goes down: the first repeat comes after 500 ms, then one every 40 ms (25 a second). Holding `a` for one second in the test gave 14 a's: the first, then 13 repeats.

From there it is the PS/2 path. `evdev::scancode` turns a key code into set-1 bytes, already written for `hid`, whose virtio keyboards send Linux key codes too. So `input::usb::key_code` maps HID usages to Linux key codes, and one table serves both.

## Interrupts: MSI, else MSI-X, else polled

The first plan was MSI only, with the whole BAR given to the driver. QEMU's `qemu-xhci` then booted **polled**: it has MSI-X and no MSI. Real controllers usually have both. The kernel's rule now:
- **MSI** if the controller has it. MSI-X stays off, so its table (often in BAR 0, among the registers) is inert, and the driver gets the whole BAR.
- Otherwise **MSI-X**, with the BAR cut short at the table, as for NVMe. A driver that could write the table could aim the device's interrupt messages, which are memory writes, anywhere. The usb crate now checks that every register it uses (the ports, interrupter 0, a doorbell per slot) lies before the cut, and refuses to start otherwise. For QEMU's controller, the cut is at 0x3000: the doorbells end at 0x2024.
- Otherwise polled: the driver looks every tick.

The log says which: `MSI-X interrupts` on QEMU's PC, `MSI interrupts` with NEC's controller under UEFI, `polled` with both switched off.

## The Rust in it

- **A trait with a default method.** `xhci::Bus` has `read`, `write`, `pause`, `ticks` and `size`, which each user implements, and `write64`, which it does not: the trait's own body writes the two halves, low first. The program and the simulator get it for nothing, and could still override it.
- **`Rc<RefCell<State>>` in the simulator.** The test hands `Controller::start` a `Sim` and keeps one itself, to check afterwards what the driver did. `Rc` (reference counted) lets both own the same state. `RefCell` lets either of them change it through a shared handle: the "only one `&mut` at a time" rule is checked when the program runs (`borrow_mut()` panics if broken) instead of when it compiles. That is right for a test double and wrong for the real driver, which owns its registers alone.
- **`[const { None }; N]`.** `Driver::devices` is an array of 8 `Option<InUse>`. `[None; 8]` only works when the element type is `Copy`, and `InUse` is not. An inline `const { None }` block is evaluated once per element instead of copied.
- **Splitting a borrow before a closure.** In `Driver::report`, `let (decoder, keys_out) = (&mut in_use.decoder, self.keys_out);` comes before `in_use.keyboard.feed(..., |code, pressed| press(decoder, keys_out, code, pressed))`. A closure that named `in_use.decoder` itself would try to borrow `in_use`, which `feed` already borrows mutably through `in_use.keyboard`. Taking the one field first, and copying the handle, leaves the closure holding only what it uses.
- **`Option::filter` as a condition.** In the kernel, `pci::msix(...).and_then(|m| m.first_in(bar)).filter(|_| use_msix)`: "where the MSI-X table starts, if it is in this BAR, *and* if MSI-X is being used". An `Option` chain reads as the sentence it means.

## Design decisions

| Decision | Chosen | Rejected, and why |
|---|---|---|
| Controller | xHCI only | UHCI/OHCI/EHCI: older PCs only. Every PC since ~2012 routes all ports through xHCI |
| Devices | Boot-protocol keyboards and mice, directly on a root port | Report protocol (needs a HID report-descriptor parser: later, for tablets and gaming mice). Hubs: a hub is a device with its own ports and its own enumeration. The next step, since many keyboards are on a hub inside the PC |
| Where it runs | One ring-3 program for the controller and its keyboards and mice | A program per device: they share the controller's rings, so one owner is simpler, and a crash costs only USB |
| Keys | Through `evdev::scancode` and the PS/2 `Decoder` | A second decoder from HID usages straight to characters: two layouts to keep in step |
| Key repeat | In the driver, 500 ms then 25 a second | In the terminal: it would need key-up events, which the terminal bytes do not carry |
| Interrupts | MSI, else MSI-X with the BAR cut, else polled | MSI-X first: it cuts the BAR for no gain when MSI exists. MSI only: QEMU's controller would always be polled |
| Devices plugged in at boot | Look at every port once at start, then events | Events only: QEMU drops the port events of a reset, which happen while the controller is stopped (Bugs) |
| DMA memory | 276 KiB: 5 pages for the controller, 4 per device for 8 devices, 32 scratchpad pages | Allocating per device: the kernel hands out DMA memory once, at boot |
| Failing endpoints | Reset Endpoint and Set TR Dequeue, then ask again; let go after 10 failures in a row | Giving up at once: a stall after an unplugged cable or a glitch is normal |

## Bugs

- **Devices plugged in at boot were never seen.** The first boot started the controller (`8 ports, 0 scratchpad pages`) and then said nothing. QEMU raises "port changed" events when the controller resets, but only delivers events while it is running. Those were raised while it was stopped, and dropped. The spec says a controller should report every port with a change bit when it starts running, but the driver should not depend on that. It now looks at every port once after starting, and a device found by both paths is attached once (`attach` checks).
- **No interrupt from QEMU's controller.** See above: `qemu-xhci` has only MSI-X. This became the MSI, then MSI-X, then polled rule, and the crate's `Bus::size` check.
- **Test script mistakes, mine:**
  - four machines booted at once from one disk image: QEMU locks an image it writes, so three failed. Each now gets a copy;
  - `!device_add,usb-kbd,id=late,bus=xhci.0`: qemu-boot turns every comma in a monitor command into a space, which split the device's options. The keyboard is now plugged in without options. It is removed by QOM path, `/machine/peripheral-anon/device[1]` (the disk is `device[0]`);
  - one QEMU's monitor socket path in my scratch directory was over 108 bytes, the limit of a Unix socket path. The trial boots now log into `build/`.
- **A simulator mistake, mine:** the host test that reads several reports read only the first. Every report needs its read queued again, so the helper now queues again until nothing comes.

## How I checked it

On the host, `usb`: 14 tests against `Sim`. The simulator:
- resets, can hold its controller "BIOS-owned" for a while or for ever, and asks for scratchpads;
- walks the command ring and the transfer rings by their cycle bits, following Link TRBs and their toggles, and writes events with its own cycle bit;
- keeps slot and endpoint contexts, and refuses a doorbell rung while the controller is stopped;
- has devices behind its ports that answer GET_DESCRIPTOR, SET_CONFIGURATION, SET_PROTOCOL and SET_IDLE. A keyboard not switched to the boot protocol sends report-protocol reports, with a report ID in front, so a driver that forgets SET_PROTOCOL reads garbage. A device can be told to refuse a request with a stall.

The tests:
- start: one reset, the BIOS's SMIs off, four scratchpads, 301 no-ops round the 256-TRB command ring;
- a BIOS that never lets go, and a controller that wants 40 scratchpads, are both refused;
- registers cut short before the doorbells: refused; cut just after the eighth doorbell: fine;
- a keyboard on a USB 2 port: enumerated, its reports read;
- 300 reports: the endpoint's 32-TRB ring nine times over and the event ring once, in order, none lost;
- endpoint 0's packet size corrected by Evaluate Context;
- USB 3 ports and 64-byte contexts;
- a keyboard and a mouse in one device, both used;
- a disk, named and let go;
- a stalled request: endpoint 0 recovered, the device still usable;
- devices coming and going;
- events arriving while a command is awaited, kept for later;
- malformed configuration descriptors refused;
- bInterval converted to xHCI's interval for each speed.

`input::usb`: 6 tests. Usages are Linux key codes, every letter and digit decodes as itself through the PS/2 decoder, reports become presses and releases, ErrorRollOver changes nothing, a held key repeats after the delay, and a mouse report is a relative move.

By booting (`make usb64-test`, new, in `make test64`): four machines at once.

| Machine | What | Seen |
|---|---|---|
| pc | `qemu-xhci` (MSI-X), USB keyboard, USB mouse, USB disk | `registers 0xfebb0000+0x3000` (cut at the MSI-X table), `MSI-X interrupts`; keyboard on port 5 and mouse on port 6, `high speed`; `46f4:0001, SuperSpeed, mass storage (class 0x08) - not driven` on port 3; `echo typed on usb` ran; `a` held for 1 s gave 14 a's; clicks at (512, 384), (612, 434), (492, 314) after moving 100 right and 50 down, then 120 up and left |
| q35 | UEFI (OVMF), NEC's controller | `MSI interrupts`, registers `0xc000004000+0x4000` (a 64-bit BAR, whole); keyboard enumerated; the line typed |
| polled | NEC's, `msi=off,msix=off` | `no interrupt (polled)`, `polled`; the line typed |
| hot | `qemu-xhci` with nothing on it | `device_add usb-kbd`: `port 5: keyboard ... ready`; `echo the late one` ran; `device_del`: `port 5: unplugged`; then `echo ps2 again` ran on the PS/2 keyboard |

Mutations:

| # | Mutation | Caught by |
|---|---|---|
| M1 | SET_IDLE sent in place of SET_PROTOCOL | 6 host tests: the simulated keyboard sends report-protocol reports |
| M2 | The Link TRB without its toggle flag | 2 host tests: the 301 no-ops, and the 300 reports |
| B1 | The driver never acknowledges the interrupt (`interrupt_seen` removed) | **Survives.** That is correct: with MSI or MSI-X, the controller clears its interrupt-pending bit itself when it sends the message (xHCI spec, IMAN.IP), and QEMU gates the next interrupt on the event ring's busy bit, which the driver does clear. Acknowledging matters only for pin interrupts, which this kernel does not use. The call stays: it is harmless, and right for pins |
| B2 | Mouse reports not sent to the display | `usb64-test`: no clicks seen at all. This also shows the clicks it checks come through the USB mouse, not the PS/2 one |
| B3 | No key repeat | `usb64-test`: "a held key did not repeat" |
| (B4) | No port scan at start | Not run as a mutation: it was the first boot's bug above, and `usb64-test` fails the same way (`never saw "keyboard (0627:0001"`) |

clippy: `usb` (all targets, after four fixes in the simulator), `input`, `user` (PC and AArch64), the kernel: clean.

`make test64`: all passed, in 14 minutes 12 seconds.

The testing note below was checked by a scripted boot with the same QEMU options as `make run64 RUN_USB=yes`: the keyboard on port 5 and the mouse on port 6; `echo hello from usb` typed on the USB keyboard; `poweroff` printing `[init] stopped usb` between `shell` and `kbd`. Then by a boot driven through QEMU's monitor: `device_add usb-kbd,id=k2,bus=xhci.0` gave `port 7: keyboard ... ready`, and `device_del k2` gave `port 7: unplugged`.

Not tested:
- **On real hardware.** Only QEMU's two controllers, `qemu-xhci` and `nec-usb-xhci`.
- **The firmware handoff.** Neither QEMU controller has the USB Legacy Support capability, so the BIOS-owned/OS-owned handshake has only been tested against the simulator. On a real PC it is the first thing that runs.
- **Scratchpads** (QEMU asks for none), and **64-byte contexts** (QEMU uses 32): simulator only.
- **Low- and full-speed devices:** QEMU's keyboard and mouse are high speed on xHCI. A real keyboard is usually low or full speed.

## What is still missing

- **Hubs.** A keyboard behind a hub, including the hub built into many PCs and monitors, is not seen. This is the most likely gap on real hardware.
- **USB mass storage**: the stick is named, not driven. That is R6.
- **The report protocol**: tablets (absolute pointers), extra mouse buttons and wheels, media keys. It needs a HID report-descriptor parser.
- **Keyboard LEDs** (Caps Lock, Num Lock): need SET_REPORT, and the decoder to say when they change.
- **More than 8 devices**, more than one controller, isochronous transfers (audio, cameras), bulk transfers (R6 adds them for storage).
- **Kernel image size:** the image now ends at 0x255000 (about 2.33 MiB), with 684 KiB left below the BIOS loader's 3 MiB.

## What I learned

- **xHCI's rings have no head and tail registers.** One bit per TRB, compared with a state each side keeps, and a toggle at the end of the ring: ownership travels with the data. It is a nicer idea than the e1000's head and tail pointers, and the Link TRB mutation shows how little it takes to break it.
- **USB is mostly enumeration.** Reading a keyboard is one queued 8-byte read. Getting there is a reset, two commands, four requests, another command and two more requests, each of which a real device may refuse or answer oddly.
- **"Should" in a spec is not "does" in an emulator.** The spec says a controller reports connected ports when it starts; QEMU's does not. Looking at the ports once costs nothing and works with both.
- **A surviving mutation can be the spec talking.** Removing the interrupt acknowledgement changed nothing, and the reason is in the spec: with message interrupts, the hardware acknowledges itself. Knowing *why* it survives is the result.
- **The interrupt choice is part of the security design.** "MSI-X with the BAR cut" and "MSI with the whole BAR" are both safe, for different reasons, and the driver has to be able to live with either.

## Testing note

**1. A USB keyboard and mouse**

```
make run64 RUN_USB=yes RUN_DISK=build/usb-disk.img
```

A new 16 MiB disk is made the first time (your `build/taaj-disk.img` is not touched). In the terminal's log:

| Look for | Meaning |
|---|---|
| `[pci] 00:05.0 USB controller: registers 0xfeb90000+0x3000, 276 KiB DMA at 0x..., MSI-X - handed to init` | the kernel found QEMU's xHCI controller and handed it on, cut short at its MSI-X table |
| `[init] started: registry display fs blk nic net shell kbd usb power; supervising` | init started `usb` |
| `[usb] xHCI 1.0 controller at 00:05.0: 8 ports, 0 scratchpad pages, MSI-X interrupts` | the driver reset and started the controller |
| `[usb] port 5: keyboard (0627:0001, high speed) ready` | the keyboard was enumerated and switched to the boot protocol |
| `[usb] port 6: mouse (0627:0001, high speed) ready` | the mouse too |

In the QEMU window (QEMU sends what you type to the keyboard that came last: the USB one):

| Do | You should see |
|---|---|
| type `echo hello from usb` and Enter | `hello from usb` |
| hold a letter key down for a second | the letter once, then repeating after half a second |
| click in the window (QEMU then gives it the mouse; Ctrl+Alt+G gives it back), move the mouse | the pointer moves |
| `poweroff` | in the log, `[init] stopped shell`, then `[init] stopped usb`, `kbd`, ..., and QEMU ends by itself |

**2. Plugging in and pulling out**

In the same machine, open QEMU's monitor with Ctrl+Alt+2 (Ctrl+Alt+1 goes back):

| Type in the monitor | In the log |
|---|---|
| `device_add usb-kbd,id=k2,bus=xhci.0` | `[usb] port 7: keyboard (0627:0001, high speed) ready` |
| `device_del k2` | `[usb] port 7: unplugged` |

**3. The tests by themselves**

```
make usb64-test
make test64
```

| Look for | Meaning |
|---|---|
| `USB worked: a keyboard typed (MSI-X, MSI under UEFI, polled), a held key repeated, a mouse clicked where it was, a disk was named, and a keyboard came and went` | the four machines of `usb64-test` |
| `test64: all passed` | every part |
