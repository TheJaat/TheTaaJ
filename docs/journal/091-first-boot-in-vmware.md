# First boot outside QEMU: VMware

*Journal entry #91 · Era 7, step R7, first part. Before trying my real PC, I booted the stick image from entry 090 in VMware Workstation, as a SATA disk. It is the first time TheTaaJ ran on anything but QEMU. It got to the shell on the first try, but with bugs that QEMU had always hidden:*

- *the loader never found a graphics mode, so there was no screen, and so no keyboard either;*
- *the kernel measured the CPU's clocks at a different speed on every boot (once at 11 GHz);*
- *under UEFI with a USB 3.2 controller, the USB driver enumerated every device it does not drive three times over.*

*All three are fixed. The first two are confirmed in VMware, and the third reproduced in QEMU and is fixed and tested there. Under UEFI, VMware's xHCI controller and USB keyboard worked with our drivers as they were. So did four CPUs, `poweroff`, and a file written before it and read after the next boot. One thing is not explained yet: on some boots the timer ticks at about half speed.*

## Why

Era 7's goal is "it boots from a USB stick on my PC and I use it for a day". Everything up to entry 090 was tested in QEMU only. QEMU is one machine, with one BIOS, one video card and one set of timers, and code can be wrong in ways that machine never shows. VMware is a cheap second machine: another BIOS (Phoenix), another video BIOS, other emulated chips, and nothing to break if it goes wrong. Whatever fails there would most likely fail on real hardware too.

## What happened

I converted the stick to a VMware disk (`qemu-img convert -O vmdk build/stick64.img build/stick64.vmdk`) and attached it as a SATA disk to a new "Other 64-bit" VM, with BIOS firmware and the default e1000 network card.

The first boot went a long way:
- our MBR, stage1 and stage2 loaded the kernel through int 13h;
- the memory, thread, user, IPC and file self-tests all passed;
- `ahci` found "VMware Virtual SATA Hard Drive", `e1000` found its Intel 82545EM, and `power` read the ACPI tables;
- `fs` found the TaajFS partition and mounted it.

Two lines were wrong:

```
framebuffer: none
[init] could not start serial: NotFound
...
[irq] APIC timer at 100 Hz (bus clock 4 MHz), TSC 3075 MHz
[selftest] timer: 10 ticks in 219 ms (expected 100)
```

And on a second boot of the same VM:

```
[irq] APIC timer at 100 Hz (bus clock 14 MHz), TSC 11122 MHz
[selftest] timer: 10 ticks in 199 ms (expected 100)
```

No CPU runs at 11 GHz.

## Bug 1: the video mode list was read from the wrong place

stage2 picks a graphics mode through the VESA BIOS (VBE):
1. Function 4F00h fills a buffer with the video card's information, including `ModeListPtr`, where its list of mode numbers is.
2. For each number in the list, function 4F01h describes the mode. The loader wants 1024x768 with 32 bits per pixel and a linear framebuffer.
3. Just before going to protected mode, 4F02h switches to the mode chosen.

`ModeListPtr` is a *far pointer*: a real-mode segment in its high 16 bits and an offset in its low 16. The loader set ES to 0 and read the list at `[es:si]`, using only the offset. In QEMU that worked by luck: SeaBIOS's video BIOS copies the list into the caller's own buffer, which is in segment 0. VMware's video BIOS keeps the list in its own ROM, at segment 0xC000. The loader read whatever was at the same offset in segment 0, found no mode, and stayed in text mode.

With no framebuffer, `init` decides that this machine's terminal is a serial port. That path exists only for AArch64 (`serial` is the PL011 terminal driver), so on x86 it found no such program, and nothing read the keyboard.

The fix (`bootloader/headers/boot/stage2/asm_includes/vesa.inc`):
- `VesaSetup` splits the far pointer: the offset goes to SI, the segment to FS.
- `VesaFindBestSuitableMode` reads the list at `[fs:si]`.
- ES stays 0, because 4F01h writes each mode's description to `ES:DI` (our `VesaModeInfo`).
- FS goes back to 0 afterwards; protected mode loads it again anyway.

While there, I stopped requiring bit 1 of the mode attributes ("extended information available"). VBE 1.2 and later always set it. A BIOS that left it clear would lose every mode for nothing, and that is the kind of thing that differs between BIOSes.

## Bug 2: the PIT is a poor ruler under VMware

The local APIC timer gives the kernel its 100 ticks a second. It counts at a "bus clock" that differs from machine to machine and is not written down anywhere standard. So since entry 006 the kernel has measured it, together with the TSC (the CPU's cycle counter, which the kernel uses as its clock):
1. start PIT channel 2 counting down 10 ms (the PIT's input clock is 1,193,182 Hz on every PC since 1981);
2. start the APIC timer counting down from its maximum, and read the TSC;
3. wait until the PIT's output, read back through port 0x61, goes high;
4. see how far the APIC timer and the TSC got in those 10 ms.

Under VMware, step 3 is unreliable. The virtual PIT's countdown is computed from the host's time whenever the virtual CPU happens to look. Port I/O leaves the virtual machine for every read, so the window comes out shorter or longer than 10 ms by a different amount on each boot. One boot measured the TSC at 3075 MHz, the next at 11122 MHz.

The fix: ask the hypervisor. A CPU running under one sets bit 31 of CPUID leaf 1's ECX ("hypervisor present") and answers CPUID leaves from 0x40000000 with the hypervisor's own information:
- leaf 0x40000000: the highest such leaf (EAX), and a 12-byte name (EBX, ECX, EDX): `VMwareVMware`, `KVMKVMKVM\0\0\0`, `Microsoft Hv`, ...
- VMware's leaf 0x40000010, "timing information": the TSC's frequency in EAX and the APIC timer's input clock in EBX, both in kHz.

Linux reads the same leaf. KVM gives it when asked to (`-cpu ...,vmware-cpuid-freq=on`).

The code:
- `kernel/kernel64/src/hypervisor.rs` (new, in the host-tested library) only decodes the answers:
  - `vendor(top)` puts the name together;
  - `has_timing(leaf1_ecx, top)` says whether the leaf exists and means "timing" (only VMware's, KVM's and QEMU's, because Hyper-V uses 0x40000010 for something else);
  - `timing(answer)` returns `(TSC Hz, bus Hz)`, or `None` for nonsense.
- `arch/x86_64/apic.rs`: `stated_frequencies()` asks the CPU (`__cpuid`) and hands the answers to those functions. The bus clock is divided by 16, since the timer counts with divide-by-16.
- `arch/x86_64/platform.rs`: the PIT is still measured, but the stated numbers win when there are any. The boot log shows both:

```
[irq] APIC timer at 100 Hz (bus clock 4 MHz), TSC 3072 MHz: stated by the hypervisor (the PIT measured 4 and 3075 MHz)
[selftest] timer: 10 ticks in 100 ms (expected 100)
```

## Not explained: the tick at half speed

At first I thought the slow tick (10 ticks in 219 and 199 ms) came from the same bad measurement, and that the fix cured it, because the next boot took 100 ms. Lining up all the boots shows that it did not:

| Boot | CPUs | Timer clock | 10 ticks took |
|---|---|---|---|
| 1, BIOS | 1 | 4 MHz, measured | 219 ms |
| 2, BIOS | 1 | 14 MHz, measured (wrong) | 199 ms |
| 3, BIOS | 1 | 4 MHz, stated | 100 ms |
| 4, UEFI | 1 | 4 MHz, stated | 99 ms |
| 5 and 6, UEFI | 4 | 4 MHz, stated | 220 ms |

Boot 1 had the right timer clock already (VMware states 65 MHz, divided by 16 that is 4 MHz) and still ticked at half speed. Boot 2 had a timer clock 3.5 times too fast, and should have ticked 3.5 times too slowly, but it was only 2 times slow. The TSC, which the test times with, came from the same bad measurement, so the two errors partly cancel. So the hypervisor fix is right, and needed (boot 2's 11 GHz would have made every TSC-timed wait 3.6 times too short), but the half-speed tick is something else.

What I know about it: the APIC timer is set right, the TSC runs at the right speed, and yet only about 46 of the 100 timer interrupts a second arrive, on some boots and not others. My guess is the host: the PC running VMware was also running my development VM, and during boots 5 and 6 that VM was running QEMU test machines at full speed. A hypervisor that is short of CPU time delivers a virtual CPU's timer interrupts late and merges them, while the TSC keeps real time. To find out, I need a boot with nothing else running on the host. The real remedy is in What is still missing: keep time with the TSC and use the tick only to switch threads, as Linux does. A late or merged interrupt then costs nothing but a slightly late thread switch.

## Four CPUs and power off

With 4 processors in the VM settings (still UEFI):

```
[acpi] 4 CPU(s), APIC ids [0, 1, 2, 3]
[smp] CPU 1 awake: APIC id 1, stack below 0xffffd80000010000
...
[smp] 4 CPUs awake: CPU 1, 2, 3 started through the page at 0x2000
[smp] counting together: 4 CPUs reached 400000 of 400000 with a lock; without one 393781, 6219 lost
[selftest] smp: threads ran on all 4 CPUs at once; 40000 hand-offs between two threads, none lost; nothing leaked
[selftest] wake-ups: a thread woken by a busy CPU ran on an idle one 62 us later (median of 40; slowest 89 us; 41 interrupts between CPUs)
```

INIT and SIPI (entry 044) started the other CPUs on a machine other than QEMU for the first time. Interrupts between CPUs were about 60 µs, faster than QEMU's 200–400 µs, because VMware runs the CPUs on real cores rather than emulating them.

Then I typed `write hello kept across a reboot`, `cat hello` and `poweroff`. `init` stopped the services in the order entry 086 set (`shell`, `usb`, `kbd`, `net`, `e1000`, `fs`, `ahci`, `display`, `registry`). `power` wrote sleep type 0 into PM1a control (port 0x444 under VMware's UEFI, 0x1004 under its BIOS), and the VM switched itself off. After the next boot, `cat hello` printed `kept across a reboot`. fs had written the file to the stick's TaajFS partition, through `ahci`, before it was stopped.

## UEFI and USB 3.2

Then I switched the same VM to UEFI (Secure Boot off: our loader is not signed), added a USB controller of the newest kind VMware offers ("USB 3.2", an xHCI controller, PCI ID 15ad:077a), and told VMware to give the VM a USB keyboard and mouse instead of PS/2 ones, with two lines in the `.vmx` file:

```
keyboard.vusb.enable = "TRUE"
mouse.vusb.enable = "TRUE"
```

What worked, unchanged:
- VMware's UEFI found `\EFI\BOOT\BOOTX64.EFI` in the stick's EFI system partition (the 0xEF partition from entry 090), and the loader gave the kernel a 1024x768 screen from GOP;
- the timer fix holds under UEFI too: `stated by the hypervisor`, 10 ticks in 99 ms;
- `[usb] xHCI 1.20 controller at 03:00.0: 8 ports, 0 scratchpad pages, MSI interrupts`: a second xHCI implementation (after QEMU's two), behind a PCIe bridge, on bus 3;
- `[usb] port 6: keyboard (0e0f:0006, full speed) ready`, and `ls` typed on it.

What did not:

```
[usb] port 5: 0e0f:0003, full speed, an input device without the boot protocol (class 0x03) - not driven
[usb] port 7: 0e0f:0002, full speed, a hub (class 0x09) - not driven
[usb] port 8: 0e0f:0002, high speed, a hub (class 0x09) - not driven
```

and then the same three lines twice more.

- **The mouse is not driven.** VMware's USB mouse is an absolute pointer (it reports where the pointer is, not how far it moved, so the host's mouse can move in and out of the window freely), and it does not offer the boot protocol. The boot protocol is the fixed 3-byte report every ordinary mouse must support for BIOSes, and it is the only one our driver reads (entry 088). Driving this one needs a HID report descriptor parser. Real mice offer the boot protocol, so this is for later.
- **Hubs are not driven** (also from entry 088). VMware's two hubs are where it plugs in USB devices passed through from the host.
- **Bug 3: each undriven device was enumerated three times.** That is a bug in `usb`, and QEMU shows it too once something undriven is plugged in (a `usb-tablet`, a `usb-hub`). The driver attaches a device whenever a port event says "connected" and it has no device on that port. A device it does not drive is not kept, so it has none. And enumerating a device resets its port, which raises another port event. So every undriven device was enumerated again, until the events happened to stop.

The fix (`user/src/bin/usb.rs`): `Driver` keeps `not_driven`, one bit per port (256 bits in four `u64`s, because xHCI numbers ports 1 to 255). A port is marked when its device is looked at and not driven, skipped by `attach` while marked, and unmarked when a port event says nothing is connected there any more. Unplugging and plugging in again looks at the new device afresh.

## The Rust in it

- **A closure that captures nothing.** `let ask = |leaf| { let r = __cpuid(leaf); Answer { ... } };` is a small function written inline. Rust works out that `leaf` is a `u32` from how `ask` is called. I call it three times, so the four-register copy is written once.
- **`?` on an `Option`.** `let (tsc_hz, bus_hz) = hypervisor::timing(...)?;` means: if `timing` returned `None`, return `None` from `stated_frequencies` right here; otherwise unwrap the pair. It is the same `?` as for `Result`, used on "maybe a value" instead of "a value or an error".
- **`bool::then_some`.** `sane.then_some((tsc, bus))` is `Some((tsc, bus))` if `sane` is true, else `None`. (`then` takes a closure instead, for when building the value costs something.)
- **Arrays of references.** `const TIMING_VENDORS: [&[u8; 12]; 3]` is three references to 12-byte arrays. `b"VMwareVMware"` is already a `&[u8; 12]`, so the names can be written as byte strings. `contains(&&vendor(top))` needs two `&`: one because `contains` takes a reference to an element, one because each element is itself a reference.
- **A bit set without a library.** `not_driven: [u64; 4]` is 256 bits. Port `p` is bit `p % 64` of word `p / 64`. `|=` with the bit sets it, and `&= !bit` clears it (`!` on an integer flips every bit). Rust wants the word index as a `usize`, so `usize::from(port / 64)` converts the `u8`: `from` exists only for conversions that cannot lose anything.
- **Keeping the instruction out of the testable part.** `__cpuid` can only run on the machine, so the code that asks stays in the kernel binary, and the code that decides goes in the library, where `cargo test` runs it on the host with the answers VMware, KVM and Hyper-V actually give.

## Design decisions

- **Not chasing the half-speed tick blind.** One more guess at a fix would be a guess. A boot on an idle host, and TSC-based timekeeping, come first.
- **Believe the hypervisor over the measurement.** The hypervisor's number is the one its virtual clocks run at. Measuring is for machines that don't say (real hardware, QEMU without KVM).
- **Only named hypervisors.** Leaf numbers above 0x40000000 mean what each hypervisor wants them to. Hyper-V's 0x40000010 is something else, and believing it would set the clock wrong in a way no test here would show.
- **Sanity limits on the answer.** KVM without the option returns zeros. A TSC outside 10 MHz–100 GHz, or a bus clock at least as fast as the TSC, is refused and the PIT measurement is used.
- **Keep measuring and log both.** If the two ever disagree badly on some machine, the log shows it without a debugger.
- **Remember "not driven" per port, not per device.** The driver gives the device back (`disable_slot`) when it does not drive it, so there is no device to remember. The port is what the next event names, and an unplug is what makes it worth a new look.
- **Not done here: an x86 terminal without graphics.** With the VESA bug fixed, a machine without a framebuffer is rare (UEFI always gives one). Making `serial` work on a PC's 16550 port is a step of its own.

## Bugs

- The far-pointer bug was in the loader from its first VESA code (it is already there in TheBootloader's first commit, January 2025). It survived every QEMU boot because SeaBIOS puts the list in segment 0.
- Bug 3 was there since entry 088, when USB arrived. `usb-test.sh` plugged in only devices the driver drives, so nothing was ever enumerated twice. Now its `pc` machine has a tablet and a hub too, and checks that each is named once.
- My first version of `stated_frequencies` wrapped `__cpuid` in `unsafe`. In current Rust, `__cpuid` is a safe function (every x86_64 CPU has CPUID), and the compiler warned about unnecessary `unsafe` blocks. I removed them.

## How I checked it

- **In VMware (the real test):**
  - Boot 1 (before both fixes): `framebuffer: none`, `could not start serial`, 10 ticks in 219 ms.
  - Boot 2 (VESA fix): `framebuffer 1024x768, 32 bpp, pitch 4096, at 0xe8000000`. Then `[display] painted the 1024x768 screen`, the terminal window, `[kbd] PS/2 keyboard ready`, and `ls` typed on the keyboard listed 39 files from the stick's partition. The timer was still wrong: TSC 11122 MHz, 10 ticks in 199 ms.
  - Boot 3 (timer fix): `TSC 3072 MHz: stated by the hypervisor (the PIT measured 4 and 3075 MHz)`, `10 ticks in 100 ms`.
  - Boot 4 (UEFI, USB 3.2, USB keyboard and mouse): `TheTaaJBoot (UEFI)`, the xHCI controller with MSI, the USB keyboard ready and typed on; the undriven devices named three times each (bug 3).
  - Boots 5 and 6 (UEFI, 4 CPUs): all four CPUs awake, every SMP self-test passed; `write`, `poweroff` (the VM switched itself off), and the file read back after the next boot. 10 ticks took 220 ms on both (see "Not explained").
- **Bug 3 in QEMU:** the PC with `qemu-xhci`, `usb-kbd`, `usb-tablet` and `usb-hub` named the tablet and the hub twice each before the fix, and once each after it.
- **QEMU without KVM** has no timing leaf, so it takes the measuring path as before: `measured against the PIT`, 10 ticks in 99–100 ms. With `-cpu max,vmware-cpuid-freq=on` it still had no leaf (the option needs KVM, which this machine lacks), so in QEMU the new path runs only through its host tests.
- **Host tests:** 3 new ones in `hypervisor.rs` (VMware believed; no hypervisor bit, Hyper-V and a too-low highest leaf not asked; zeros and impossible values refused). 197 library tests in all.
- **Mutation:** removing the vendor check made `others_are_not_asked` fail (Hyper-V would have been believed).
- **`usb64-test`:** its `pc` machine now has a tablet and a hub, and fails unless each is named exactly once. With the old `usb.rs` put back, it fails: `"an input device without the boot protocol (class 0x03) - not driven" was logged 2 times, not once`. With the fix, `usb64-test passed`. The first version of the check failed with the fix too (logged 0 times): `qemu-xhci` has only 4 USB 2 ports by default, and with a fifth device QEMU silently adds a hub of its own and puts the device behind it, where our driver cannot see it. The test's controller now has 8 ports of each kind (`p2=8,p3=8`).
- **Boot tests:** `boot64-test` and `stick64-test` pass. QEMU still gets its 1024x768 screen through the changed VESA code, by the CD and by the stick (BIOS and SATA machines).
- `make test64`: all passed, in 13 minutes 52 seconds.

## What is still missing

- **The real PC**, the rest of R7.
- **USB pointers without the boot protocol** (VMware's absolute mouse, tablets, touch screens): a HID report descriptor parser.
- **USB hubs**, so devices behind one (VMware's passed-through ones, a keyboard on a monitor's hub) work.
- **A USB stick in VMware:** VMware attaches its virtual disks only as IDE, SCSI, SATA or NVMe, never as USB. A real stick passed through from the host would work, but only under UEFI (VMware's BIOS cannot boot from USB), and it sits behind one of VMware's hubs, which we do not drive yet.
- **No screen on x86:** the PC's serial port as a terminal (`serial` speaks only PL011).
- **No interrupts for the 82545EM:** it has no MSI, and TheTaaJ does not route legacy PCI interrupts (that needs ACPI's `_PRT`), so `e1000` polls it.
- **Time from the TSC, not from counting ticks.** `sleep` and the uptime count timer interrupts, so every late or merged interrupt makes time run slow (the half-speed tick). Counting with the TSC, and using the tick only to switch threads, would make that harmless.
- **Real hardware's clocks:** CPUID leaves 0x15 and 0x16 give the TSC's and the crystal's frequency on recent Intel CPUs, without a hypervisor. The PIT is still used there, or the HPET, which is a better ruler and is not used yet.

## What I learned

- A test machine that never varies hides whole classes of bugs. The far-pointer bug was almost two years old and passed every test, because one BIOS happened to put the mode list where the wrong code looked.
- A segment:offset pointer is two numbers. Using half of one is the real-mode version of a dangling pointer.
- In a virtual machine, the old timer chips are simulated from the host's clock whenever the guest looks. They are good enough to tick by, and poor for measuring a short window. Hypervisors say what their clocks run at for exactly this reason.
- An "ignore this" decision needs a memory, or it gets made again. Here the act of looking (enumerating resets the port) caused the event that made the driver look again.
- A fix that is followed by the right result is not proven by it. Only putting all six boots in one table showed that the half-speed tick came and went independently of my fix.
- Showing both numbers in the log (stated and measured) made the difference visible right away: 4 and 3075 MHz measured on that boot, against 3072 MHz stated.

## Testing note

**1. In VMware Workstation** (or any VMware)

```
make stick64
qemu-img convert -f raw -O vmdk build/stick64.img build/stick64.vmdk
```

Copy `build/stick64.vmdk` to the host. If VMware's shared folder is set up: `sudo vmhgfs-fuse .host:/ /mnt/hgfs -o allow_other`, then `cp build/stick64.vmdk /mnt/hgfs/<share>/`.

Make a new VM: "Other 64-bit", 1–2 GB of memory, an existing disk (`stick64.vmdk`) attached as **SATA** (not SCSI: TheTaaJ has no driver for VMware's SCSI controllers), BIOS firmware. To see the log, add a serial port that writes to a file.

| Look for | Meaning |
|---|---|
| `framebuffer    1024x768, 32 bpp, ...` in the boot info | the loader found the video BIOS's mode list (bug 1) |
| `[irq] APIC timer at 100 Hz (bus clock 4 MHz), TSC ... MHz: stated by the hypervisor (the PIT measured ...)` | the clock frequencies came from CPUID leaf 0x40000010 (bug 2) |
| `[selftest] timer: 10 ticks in ... ms (expected 100)` | about 100 on boots 3 and 4; 200–220 on others (see "Not explained"). The test passes up to 300 |
| `[ahci] ... disk on port 0: VMware Virtual SATA Hard Drive, 53848 sectors` | VMware's AHCI controller works with our driver |
| `[fs] TaajFS partition: sectors 20480 to 53247` | the stick's TaajFS partition |

On the screen, a terminal window opens. Type `ls`: 39 files and 4 directories.

**2. The same VM under UEFI, with USB**

Power the VM off, then:
- Settings → Options → Advanced: firmware **UEFI**, Secure Boot off;
- Settings → Add… → USB Controller → **USB 3.2** (1.1 and 2.0 are UHCI and EHCI, which TheTaaJ does not drive);
- in the VM's `.vmx` file: `keyboard.vusb.enable = "TRUE"` and `mouse.vusb.enable = "TRUE"`.

| Look for | Meaning |
|---|---|
| `  loader         TheTaaJBoot (UEFI)` | VMware's UEFI ran our loader from the EFI system partition |
| `[usb] xHCI 1.20 controller at 03:00.0: 8 ports, 0 scratchpad pages, MSI interrupts` | VMware's xHCI controller |
| `[usb] port 6: keyboard (0e0f:0006, full speed) ready` | VMware's USB keyboard: typing goes through it |
| `[usb] port 5: 0e0f:0003, ..., an input device without the boot protocol (class 0x03) - not driven` | the absolute mouse, which needs a report parser - **once**, not three times |
| `[usb] port 7: 0e0f:0002, ..., a hub (class 0x09) - not driven` (and port 8) | VMware's hubs, once each |

Type `ls`: the keys come through USB now.

**3. Four CPUs and power off**

Settings → Processors: 4 (or 2). Boot, then type `write hello kept across a reboot`, `cat hello`, `poweroff`.

| Look for | Meaning |
|---|---|
| `[smp] 4 CPUs awake: CPU 1, 2, 3 started through the page at 0x2000` | INIT/SIPI woke VMware's CPUs |
| `[selftest] wake-ups: ... ran on an idle one ... us later` | interrupts between CPUs; about 60 µs in VMware |
| `[init] stopped shell`, ..., `[init] stopped fs`, `[init] stopped ahci`, ... | services stopped in order, fs before the disk's driver |
| `[power] switching off: sleep type 0/0 into PM1 control (ACPI S5)` | and the VM switches itself off |

Boot again and type `cat hello`: `kept across a reboot`.

**4. In QEMU**

```
make run64
```

The log says `TSC ... MHz: measured against the PIT` (QEMU without KVM gives no timing leaf), `10 ticks in 99 ms` or `100 ms`, and the screen is 1024x768 as before.

**5. Undriven USB devices, in QEMU**

```
make run64 RUN_USB=yes RUN_USB_ARGS="-device qemu-xhci,id=xhci -device usb-kbd,bus=xhci.0 -device usb-tablet,bus=xhci.0 -device usb-hub,bus=xhci.0"
```

The log names the tablet (`an input device without the boot protocol`) and the hub once each, and the USB keyboard types.

**6. The tests by themselves**

```
cd kernel/kernel64 && cargo test --lib --target x86_64-unknown-linux-gnu hypervisor
make boot64-test stick64-test usb64-test
make test64
```

| Look for | Meaning |
|---|---|
| `test result: ok. 3 passed` | CPUID answers decoded: VMware believed, Hyper-V and nonsense refused |
| `boot64-test passed`, `stick64-test passed` | the VESA change still finds QEMU's mode |
| `usb64-test passed` | among the rest, the tablet and the hub are named once each |
| `test64: all passed` | every part |
