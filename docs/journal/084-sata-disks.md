# SATA disks: an AHCI driver, so TheTaaJ's disk can be the kind every PC has

*Journal entry #84 · Era 7, step R1. Until now TheTaaJ found its disk only as a virtio device, which exists only inside QEMU and similar virtual machines. Now the kernel also hands on any AHCI controller, the standard SATA controller of every PC since about 2005. A new driver, `ahci`, drives the first disk on it and serves the same disk protocol as `blk`, so the file service cannot tell the two apart. On QEMU it works on an AHCI controller added to the old PC, and on the q35 machine's own, the one in its chipset, under UEFI. That is the first of Era 7's steps toward a real PC.*

## Why

Era 7's goal is booting TheTaaJ from a USB stick on my own PC and using it for a day. Everything TheTaaJ stores lives on one disk, and so far that disk had to be a virtio block device: a made-up device that only virtual machines offer. VirtualBox and VMware don't offer it for disks, and no real PC has one. What they all have is SATA behind an AHCI controller: the chipset's own on a real machine, an emulated ICH9 or similar in a virtual one. So AHCI comes first. NVMe, the other disk a modern PC may have, comes next (R2), on the same plumbing.

## What it is

```
 kernel (TheKernel)          init                 ahci (user/src/bin)          fs
 ------------------          ----                 -------------------          --
 PCI scan: class 01.06.01 ─► "pci" + description ─► start: ahci::Disk::start ─► abi::block requests
 BAR 5 -> memory object       registers: READ|WRITE   (ahci crate: the whole      (the same as with blk:
 33 pages of DMA memory       starts "ahci" as the    sequence, host-tested)       it cannot tell)
 MSI (or MSI-X) -> a line     disk driver             block_service::serve
```

- **The kernel knows only PCI.** A function whose class is 01.06.01 (storage, SATA, AHCI 1.0) is offered, whoever made it. Its registers are BAR 5's memory, handed on as a memory object mapped uncached. It gets DMA memory and an interrupt. Nothing in the kernel knows what a port, a FIS or a sector is.
- **MSI, new in the kernel.** Virtio disks use MSI-X: a table of messages in a BAR. SATA controllers usually have plain MSI instead: one message, set up in configuration space itself. `pci::msi` and `pci::enable_msi` find and program it. Any future PCI driver can use it.
- **The `ahci` crate** (TheTaaJ, beside `virtio/`): the register map, the layouts, the commands, reading the disk's IDENTIFY answer, and `Disk`, the whole sequence that drives one disk. It is pure logic against a trait, so it is tested on the host against a simulated controller.
- **The `ahci` program**: gives the crate the real registers, the real DMA memory and a way to wait, then serves the disk.
- **`taaj::block_service`**: the request loop that used to be inside `blk`, moved out so that both drivers share it.
- **init** starts `ahci` instead of `blk` when the disk it was handed is an AHCI controller.

## How AHCI works

A SATA disk understands ATA commands: "read 8 sectors from sector 4096", "tell me about yourself". On the cable they travel as **FISes** (frame information structures), small packets. Between the CPU and the cable sits the **HBA** (host bus adapter). AHCI is Intel's standard for what an HBA looks like to software:

- **Registers in memory** (BAR 5, the "ABAR"): a few for the whole controller (capabilities, global control, which ports exist), then 0x80 bytes for each of up to 32 ports. A port is one cable, so one disk.
- **Commands in memory.** Each port reads its commands by DMA from memory the driver gives it. It is the same idea as a virtqueue, with other shapes:

```
 DMA memory (33 pages)
 0x0000  command list      32 headers of 32 bytes; only slot 0 is used
 0x0400  received FIS      where the HBA writes what the disk answered
 0x0800  command table     the command FIS (20 bytes, sent to the disk as is),
                           then at 0x880 one PRD: "the data is at physical X, N bytes"
 0x1000  data              128 KiB: what a read brings or a write sends
```

To run a command, the driver:
1. writes the FIS into the table;
2. writes the PRD;
3. writes slot 0's header: the FIS length, the direction, one PRD, and where the table is;
4. sets bit 0 of the port's **PxCI** (command issue) register.

The HBA sends the FIS, moves the data, clears the bit when the disk says it is done, and raises an interrupt. On an error it sets **TFES** (task file error) in the port's interrupt status, leaves the bit in PxCI set and stops processing. The disk's status and error bytes are then in **PxTFD**.

Taking a disk over (`Disk::start`):
1. **The BIOS handoff**, if CAP2 says the firmware may own the controller: ask for it (OOS) and wait until the BIOS lets go.
2. **AHCI mode on** (GHC.AE): a controller may start in an IDE-like mode.
3. **Find the first port with a disk.** Its link status must say "present, link up, active", and its signature must be 0x101 (an ATA disk; 0xEB140101 is a CD drive).
4. **Stop the port, then wait until it confirms.** That means clearing ST and waiting for CR, then clearing FRE and waiting for FR, within 500 ms. Only then may its lists move. This step matters more than it looks: see "What I learned".
5. **Give the port our command list and FIS area,** and clear its error and status bits.
6. **Start it:** first let it receive FISes (FRE), then wait until the disk is neither busy nor wanting data (if it stays busy: a COMRESET, holding the link in reset for a moment). Then set ST, and switch on the port's interrupts and the controller's.
7. **IDENTIFY:** 512 bytes back. They give the number of sectors (words 100-103 for 48-bit LBA, else 60-61), whether the disk knows the 48-bit commands (word 83 bit 10), the sector size (word 106, words 117-118) and the model name (words 27-46, two characters a word, the first in the high byte).

Reads and writes are READ DMA EXT and WRITE DMA EXT (0x25, 0x35). An old disk without 48-bit LBA gets READ DMA and WRITE DMA (0xC8, 0xCA): at most 256 sectors a command, and LBA bits 24-27 in the device register.

### Waiting, and not missing the end

```rust
loop {
    let status = self.hba.read(at(port::IS));
    if status != 0 {
        self.hba.write(at(port::IS), status);     // write 1 to clear
        self.hba.write(hba::IS, 1 << self.port);
    }
    if status & port::IS_ERRORS != 0 { /* the disk's error: recover, report */ }
    if self.hba.read(at(port::CI)) & 1 == 0 { return Ok(()); }   // done
    if self.hba.ticks() >= deadline { /* no answer: recover */ }
    self.hba.pause();
}
```

The status is cleared **before** PxCI is looked at. A command that ends after the look sets the status again and sends another interrupt. One that ended before is seen by the look. The other way round, it could end in between and never be noticed. The driver's `pause` waits on the interrupt handle *with a deadline of one tick* (`object_wait_until`), so even a lost interrupt costs 10 ms, not a hang. The kernel counts interrupts, so one that arrives before the wait begins makes the wait return at once.

After an error or a timeout, `recover` stops the port, clears its errors, resets the link if the disk is stuck busy, and starts it again. The request fails with `IO_ERROR`, and the next one works.

## The Rust in it

- **A trait as the seam between logic and hardware.** `Hba` has four methods: `read`, `write`, `pause` and `ticks`. `Disk<H: Hba>` is *generic*: the compiler makes a separate copy of `Disk`'s code for each type it is used with (this is called *monomorphization*). In the driver, `H` is `Registers` (volatile reads of mapped memory). In the tests, it is `Sim` (a simulated controller). Neither copy pays for the other, and there are no function pointers.
- **Sharing state with a test: `Rc<RefCell<State>>`.** `Disk::start` takes its `Hba` *by value*: it owns it from then on. The test still wants to look inside the simulator afterwards. `Rc` is a reference-counted pointer: `sim.clone()` makes a second owner of the same `State`. `RefCell` moves the borrow rules (many readers *or* one writer) from compile time to run time, so both owners can borrow it in turn. `borrow_mut()` while another borrow is alive would panic. That is a fine failure in a test, and it is why each method's borrow ends before the next begins.
- **`&mut impl Device`** in `block_service::serve` is shorthand for a generic parameter (`fn serve<D: Device>(device: &mut D, ...)`): one copy of the loop for `blk`'s `Disk` and one for `ahci`'s `Sata`.
- **Volatile, everywhere the HBA can see.** The compiler assumes that memory changes only when the program changes it. The HBA writes the DMA memory and the registers behind its back. `read_volatile` and `write_volatile` tell the compiler that every access matters: none may be merged, skipped or moved past another. A `fence(SeqCst)` before setting PxCI makes sure the table and header are in memory before the controller is told to read them. x86 keeps stores in order anyway; AArch64 would need it.
- **`unsafe` with a reason.** Each raw-pointer access has a `// SAFETY:` comment that says why it stays inside the mapping. `Dma::new` is itself `unsafe fn`: its caller promises that the pointer, size and physical address describe the same memory, which nothing else uses.

## Design decisions

| Decision | Chosen | Rejected, and why |
|---|---|---|
| Where AHCI knowledge lives | A crate used by the driver (and by init, for the class numbers); the kernel matches the PCI class only | Recognising ports or disks in the kernel: the kernel stays mechanism, and NVMe and e1000 will need the same "memory BAR + MSI" and nothing more |
| One driver or two | A separate program, `ahci`; the request loop moved into `taaj::block_service` for both | A third "transport" inside `blk`: virtio and AHCI share nothing below the request loop, and the loop was the only part worth sharing |
| Interrupts | MSI, set up by the kernel, which is new; the driver waits with a one-tick deadline | Polling only: it works (the driver falls back to it) but takes a CPU's time; pin interrupts: they need routing through ACPI's AML, which the kernel does not read |
| Taking the controller over | Stop the one port and give it new lists | A whole-controller reset (GHC.HR): it resets every port's link, and a real disk can take seconds to come back |
| Commands in flight | One, in slot 0 | Native command queueing (up to 32): faster on a real disk, and later |
| A disk without 48-bit commands | READ DMA / WRITE DMA, 256 sectors at most | Refusing it: the code is small, and the tests cover it |
| The device description | `PciDeviceInfo` grew (28 to 48 bytes): class, a memory register range, MSI | A new message type: one description of a PCI function is simpler for init, and its length still differs from `MmioDeviceInfo`'s (37), which is how a driver tells them apart |
| Which disk, with several | The first: the first PCI disk the kernel offers (virtio or AHCI), and the first port with a disk | Several disks: fs knows one disk, and so far that is all it needs |

## Bugs

- **The driver could not map its registers.** The first boot said `[ahci] cannot map the controller's registers: AccessDenied`, five times, and init gave up. init grants the disk's registers handle `READ | WRITE` only for a memory-mapped virtio disk (AArch64's); a PCI disk's registers had always been I/O ports, which take no rights. It now asks whether the registers are in memory, for either kind.
- **A control character in a disk's name was let through.** The model name was trimmed to its last *printable, non-space* character. That skipped the bad character instead of finding it, so the check after it never saw it. The host test of a name with a 0x01 in it found this. Padding is now spaces or NULs only, and anything else counts as part of the name and must be printable.
- **The BIOS loader cannot boot q35.** On the q35 machine, BIOS boot printed nothing at all. Stage 2 of TheTaaJ's BIOS loader reads the CD through the old IDE ports (`stage2/ata.c`). q35 has no IDE controller: its CD drive sits on the AHCI controller. This is not fixed here (it is the bootloader's, see "What is still missing"). Under UEFI the firmware reads the CD, and q35 boots. The test uses that.

## How I checked it

On the host:
- `ahci`, 17 tests, against `Sim`. Sim is a controller with a disk. Its registers behave as the standard says: status bits cleared by writing 1, CR following ST, FR following FRE, COMRESET. A command engine reads slot 0's header, table and PRD out of the same memory the driver wrote them to, checks every field the standard constrains, and moves data between that memory and a disk in a `Vec`. It panics at the first rule broken: lists moved while the port runs, a slot issued twice, an odd PRD length, a direction that does not match the command. The tests:
  - the first disk is found past a CD drive;
  - set-up with and without an interrupt;
  - sectors written and read back, including the last sector;
  - a port the firmware left running;
  - a port that never stops, given up after 500 ms;
  - a disk error reported, then the port recovers;
  - a sector past the end;
  - a command that never ends, which times out and recovers;
  - a disk stuck busy, which gets a link reset;
  - no disk, a disk on a port not implemented, a sleeping link;
  - DMA memory above 4 GiB without 64-bit addressing;
  - a 28-bit disk;
  - the BIOS handoff, given and refused;
  - a driver restarted with a command still in flight;
  - the FIS, header and PRD byte layouts;
  - port signatures;
  - IDENTIFY answers, good and bad.
- `abi`: `PciDeviceInfo` round trip (26 tests). Kernel: `pci::msi`/`enable_msi` against the simulated PCI bus, including QEMU's ICH9 layout and a 64-bit maskable one (191 tests).

By booting (`ahci64-test`, new, in `make test64`; 41 s alone), two machines at once, with **no virtio disk**:
- **pc** (BIOS), with an AHCI controller added: an empty CD drive on port 0 and the disk on port 2:
  - the kernel's `[pci] ... AHCI controller: registers ..., 132 KiB DMA ..., MSI - handed to init`;
  - `disk on port 2: QEMU HARDDISK, 8192 sectors (4 MiB), MSI interrupts`;
  - `disk check`, all 9 (256 sectors at once against one at a time, and the refusals);
  - `cat welcome`, `cat docs/readme`, and `wc long` (300 lines, read in many sectors at once);
  - `wc` itself loaded from the disk;
  - `write saved kept`, and `cp long copy` (written in many sectors at once);
  - `disk stall 3`: init stops ahci after 2 s and starts it again, the new one takes the controller over from the dead one, and `disk read 0` (TaajFS's `JAFS`), `disk check` and `cat saved` all work through it.
  - On the host afterwards, `saved` holds `kept` and `copy` is byte for byte `long`.
- **q35** (UEFI): the chipset's own controller at 00:1f.2, with the boot CD on port 2, which the firmware had just used:
  - `disk on port 3`;
  - `disk check`, all 9;
  - `cat welcome`;
  - `write saved q35`, read back on the host.

Mutation experiments, on the host (`cargo test` in `ahci/`):

| Mutation | Result |
|---|---|
| M1: the port not stopped before its lists move | 3 tests fail ("the lists moved while the port was running") |
| M2: the status bits never cleared | 1 fails |
| M3: a CD drive taken for a disk | 4 fail |
| M4: `recover` leaves the port stopped | 2 fail |
| M5: the PRD's length not stored minus one | 12 fail |
| M6: the header's direction bit dropped | 3 fail |
| M7: a 28-bit LBA's top four bits dropped | 1 fails |
| M8: the BIOS handoff skipped | 1 fails |

And three of them on QEMU (`make ahci64-test`):

| Mutation | Result |
|---|---|
| B1 = M1 | caught: on the pc machine, IDENTIFY never comes back, the watchdog restarts ahci again and again, and fs finds no disk |
| B2 = M6 | **survives**: QEMU takes the direction from the command, not from the header bit. Only the simulator catches it |
| B3 = M3 | caught: ahci picks the empty CD drive on port 0, its IDENTIFY fails, no disk |

clippy: `ahci` (all targets), `abi`, the kernel, `user`; clean. `user` builds for AArch64 too; `ahci` is built there, but nothing starts it.

`make test64`: every part passed, the first time, in 858 s, `ahci64-test` among them.

The testing note below was run by a scripted boot on a fresh disk (`build/note-sata.img`), both parts, and its outputs are copied from that boot.

Not tested:
- **On real hardware, VirtualBox or VMware.** Only QEMU's ICH9 AHCI has run this driver. The VMware steps in the testing note are untested.
- **Polling.** QEMU's AHCI always has MSI, so the driver's polled path ran only against the simulator.
- **The BIOS handoff, COMRESET, a timeout, a disk error, a 28-bit disk:** only against the simulator. QEMU's disk does not fail on request.

## What is still missing

- **The BIOS loader reads the CD through IDE ports.** So a machine whose CD drive is on SATA (q35; probably VMware or VirtualBox with a SATA CD) boots only by UEFI. Teaching stage 2 to read through the BIOS (int 13h extensions) instead would fix it.
- **No flushing.** A disk may keep written sectors in its own cache, and say "done" before they are on the platters. TaajFS's copy-on-write commits assume that a write that is done is safe; after a power cut, a commit block could reach the disk before the blocks it points to. FLUSH CACHE EXT before and after a commit, as a new block request that fs sends, would close this, for virtio too (it has the same gap).
- **One command at a time.** NCQ would let a real disk reorder 32 requests; fs sends one at a time anyway, for now.
- **One disk.** A second port's disk, port multipliers, hot plug, and CD drives (ATAPI) are not driven.
- **Power states.** A port whose link is asleep is passed over, not woken.
- **4Kn disks** (4096-byte logical sectors) are refused: the block protocol is built on 512-byte sectors.
- **DMA is not confined.** As with every driver here, a broken `ahci` could have the controller write anywhere in memory; an IOMMU would stop it (the note at the top of `kernel64/src/devices.rs`).
- **AArch64 has no PCI yet,** so `ahci` is built there but never runs.

## What I learned

- **The firmware leaves things running.** SeaBIOS uses the AHCI controller itself, to look for something to boot, and leaves the port started with its own command list in memory the kernel now owns. Without the stop, QEMU went on reading the firmware's old list, and IDENTIFY never came back (mutation B1). The standard's "stop the port before moving its lists" is not a formality: the first machine I tried breaks without it.
- **An emulator forgives what hardware may not.** QEMU ignores the header's direction bit (B2). A test against QEMU alone would never have noticed it was wrong. The simulator checks the rules as written, which is what a real controller is entitled to rely on.
- **Small, generic mechanisms pay off.** The kernel's part of a whole new kind of disk was a memory BAR as a memory object and MSI: two mechanisms, each reusable by the next driver. Everything about SATA stayed in user space and in a crate that runs on the host.

## Testing note

**1. The PC, with the disk on SATA**

```
make run64 RUN_DISK_BUS=sata RUN_DISK=build/sata-disk.img
```

A new 16 MiB disk is made the first time (your `build/taaj-disk.img` is not touched). In the terminal's log:

| Look for | Meaning |
|---|---|
| `[pci] 00:03.0 AHCI controller: registers 0x...+0x1000, 132 KiB DMA at 0x..., MSI - handed to init` | the kernel found the controller and set up its MSI |
| `[init] started: registry display fs ahci nic net shell kbd; supervising` | init started `ahci`, not `blk` |
| `[ahci] AHCI 1.0 controller at 00:03.0, 6 ports; disk on port 0: QEMU HARDDISK, 32768 sectors (16 MiB), MSI interrupts` | the disk's IDENTIFY answer |

In the QEMU window:

| Type | You should see |
|---|---|
| `disk` | `disk: 32768 sectors of 512 bytes (16384 KiB)` |
| `disk check` | `disk check: all 9 checks passed` |
| `write sata hello` | `sata: 6 bytes` |
| `cat sata` | `hello` |
| `wc sata` | `sata: 1 line, 1 word, 6 bytes` |
| `disk stall 3` | `disk: the driver will answer nothing for 3 s`; in the log, `[init] ahci has not answered for 2 s - stopping it`, `[init] ahci ended with code -1001 - restarting it`, and the `[ahci] AHCI 1.0 controller ...` line again |
| `disk read 0` | `0000  4a 41 46 53 04 00 00 00 ...  JAFS............` |
| `cat sata` | `hello` |

Close QEMU, then on the host:

| Type | You should see |
|---|---|
| `tools/tfstool-rs/target/release/tfstool get build/sata-disk.img sata` | `hello` |

**2. A q35: the chipset's own AHCI controller, under UEFI**

```
qemu-system-x86_64 -machine q35 -m 512M -bios /usr/share/ovmf/OVMF.fd -cdrom build/image64.iso -serial stdio \
    -drive file=build/sata-disk.img,format=raw,if=none,id=d0 -device ide-hd,drive=d0,bus=ide.3
```

| Look for / Type | You should see |
|---|---|
| (the log) | `[pci] 00:1f.2 AHCI controller: ...` and `[ahci] AHCI 1.0 controller at 00:1f.2, 6 ports; disk on port 3: QEMU HARDDISK, 32768 sectors (16 MiB), MSI interrupts` (the boot CD is on port 2) |
| `cat sata` | `hello`, written in part 1 |
| `disk` | `disk: 32768 sectors of 512 bytes (16384 KiB)` |

**3. VMware (not tested by me)**

```
qemu-img convert -O vmdk build/sata-disk.img build/sata-disk.vmdk
```

A new VMware VM: firmware **UEFI** (the BIOS loader cannot read a SATA CD, see "What is still missing"), `build/image64.iso` as its CD, and `sata-disk.vmdk` as an existing disk on its **SATA** controller. Then the same steps as part 1. Its log goes to the VM's serial port, if one is added and sent to a file.

**4. The tests by themselves**

```
make ahci64-test
make test64
```

| Look for | Meaning |
|---|---|
| `ahci drove the disk on a PC's added AHCI controller (port 2, past a CD drive) and on a q35's own (port 3, past the boot CD): ...` | both machines |
| `test64: all passed` | every part |

Automated: make ahci64-test
