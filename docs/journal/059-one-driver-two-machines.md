# One driver, two machines

*Journal entry #59 · Era 3, step 3g, part 2. The AArch64 kernel hands a disk to a program: its registers, memory for DMA and its interrupt, each as a capability. The program is `blk`, the same driver that runs the PC's disk, now with a second way of reaching a device. A client tests it, and afterwards the image file is read from outside the machine.*

```
[virtio] 0xa003e00 block device: registers in the page at 0xa003000, 64 KiB DMA at 0x40de6000, interrupt 79, off until its driver waits - ready for a driver
[blk] virtio disk at 0xa003e00: 6143 sectors (3071 KiB), queue of 1024, interrupts
[blkcheck] 348 checks, all passed: 6143 sectors, 33 written at 100 and the last one
[selftest] disk: blk and blkcheck, twice on one device - every check passed; 338 interrupts passed to the driver; nothing leaked
```

---

## Why

Entry 58 ended with the kernel able to say "there is a disk at 0xa003e00, interrupt 79" and nothing more. A driver in this system is a program, and a program can touch nothing it was not handed. So three things had to exist before a single sector could be read:

1. a way for a program to reach the device's registers,
2. memory the device can read and write, whose physical address the program knows,
3. the device's interrupt, as something a program can wait for.

On the PC all three exist since entry 14. Two of them carried over almost unchanged. The third - the interrupt - is where the two machines really differ, and it is most of this entry.

## What a driver is given

| | PC (entry 14) | AArch64 `virt` (now) |
|---|---|---|
| Registers | an `IoPorts` capability for the device's port range | a memory object over the page the registers are in |
| DMA memory | a contiguous memory object, 64 KiB, and its physical address | the same |
| Interrupt | an `Interrupt` object: MSI-X message 0 → a vector | an `Interrupt` object: a shared line of the interrupt controller |
| Description | `PciDeviceInfo`, 28 bytes | `MmioDeviceInfo`, 37 bytes |

Registers in memory needed nothing new in the kernel. A memory object over fixed physical pages already existed for the screen (`MemoryObject::device`); here it is made with the "device memory" kind, so every access really reaches the device, in order:

```rust
let registers = MemoryObject::device(slot.registers.start, slot.registers.size, Flags::NO_CACHE | Flags::WRITE_THROUGH);
```

The two descriptions have different lengths on purpose. The driver receives some bytes and asks each type "are these you?":

```rust
let started = if let Some(info) = PciDeviceInfo::from_bytes(described) {
    start_on_pci(&info, registers, dma, interrupt)
} else if let Some(info) = MmioDeviceInfo::from_bytes(described) {
    start_in_memory(&info, registers, dma, interrupt)
} else { … };
```

`from_bytes` returns `Option`: `None` for any length but its own. `if let Some(info) = …` runs the branch only when there is a value, and names it. No tag byte, no version field - and a host test pins down that neither type accepts the other's bytes.

## The interrupt: a message against a wire

On the PC the disk interrupts with MSI-X: the device *sends a message*, once, and it is over. Nothing has to be answered. The kernel counts it and wakes the driver.

A virtio-mmio device has a *wire*. It raises it when it has finished a request, and keeps it raised until the driver writes "seen" to a register (`INTERRUPT_ACK`). That is a level interrupt: the line says "I still need attention", continuously.

Now put that together with a driver that is a program:

```
device raises the line
  → CPU takes the interrupt, kernel handler runs
  → handler wakes the driver (a thread, not running yet)
  → handler returns
  → the line is still raised: the driver has not run, so nobody has told the device
  → CPU takes the interrupt again. And again. The driver never runs.
```

A kernel with the driver inside it would write the acknowledgement in the handler. Mine cannot: the kernel knows nothing about virtio, and should not learn.

What it can do without knowing the device: **switch the line off at the interrupt controller**, and switch it on again when the driver comes back to wait.

```
            kernel                              driver (a program)
 interrupt 79 ──► gic::disable(79)
                  line.fire()  ─── wakes ────►  returns from interrupt_wait
                  end of interrupt              reads INTERRUPT_STATUS, writes INTERRUPT_ACK
                                                takes the finished request from the queue
                  gic::enable(79) ◄── calls ──  interrupt_wait again
```

In code, the handler's new branch:

```rust
} else if gic::has_shared(id) && {
    gic::disable(id);
    crate::interrupts::source_fired(id)
} {
```

and in `InterruptLine::wait`, at the top of its loop:

```rust
let source = self.source.load(Ordering::Relaxed);
if source != 0 {
    crate::arch::rearm_interrupt(source);
}
```

`InterruptLine` gained one field, `source`: the controller's number for the line, or 0 for "nothing to switch" (every line on the PC). The kernel needs no knowledge of the device, and the driver cannot forget to re-enable: asking for the next interrupt *is* re-enabling.

### What the tree says, and what is true

QEMU's device tree says the virtio lines are **edge**-triggered (entry 58 noticed this). The device behaves as level. I follow the tree - it is the machine's own description - and set the controller's configuration register accordingly:

```rust
pub fn trigger(id: u32) -> (usize, u32) {
    (distributor::CONFIG + (id as usize / 16) * 4, 0b10 << ((id % 16) * 2))
}
```

Sixteen interrupts to a register, two bits each, the higher one meaning edge. With edge, the controller notices only the moment the line rises, so the storm above cannot happen even without the switching off. Which means on this machine, as described, my switching-off is not needed - and a test cannot tell whether it works. I did not want to carry untested code for "real hardware later", so there is a boot with an edited tree that says *level* for the disk's line (see the checks below). There the switching off is the only thing between the kernel and an endless interrupt.

### The order in the driver

After an interrupt the driver does two things: tells the device "seen", and looks in the queue for finished requests. The order matters:

```rust
if taaj::interrupt_wait(line).is_err() { return block::IO_ERROR; }
// Before the loop looks at the queue again.
self.transport.interrupt_seen();
```

"Seen" first, then look. A request that finishes *after* the look raises the line afresh. One that finished *before* is found by the look. The other way round - look, then "seen" - a request finishing between the two would be acknowledged without ever having been looked for.

### Where it goes: a third thing per controller version

A shared interrupt must be told which CPU to go to. GIC version 2: one byte per interrupt, a bit per CPU (`TARGETS`). Version 3: a 64-bit register per interrupt holding the CPU's *name* - the affinity fields of its `MPIDR` register (`ROUTE`, at 0x6000 + 8·id). `gic::route(mpidr)` masks out the bits that are not the name; both are host-tested.

## One driver, two transports

Of `blk`'s 384 lines only the set-up sequence and one notify touched the device's registers. Those went behind an `enum`:

```rust
enum Transport {
    #[cfg(target_arch = "x86_64")]
    Ports(u16),
    Memory(*mut u32),
}
```

`#[cfg(target_arch = "x86_64")]` on a *variant* removes it from the type on other CPUs - AArch64 has no I/O ports, and there the enum has one variant and the `match`es one arm. The queue (`virtio::Queue`), the request format, the service loop and both channels are untouched: the legacy memory-mapped interface lays its queue out exactly as legacy PCI does.

Setting up differs in three ways:

| | legacy PCI | legacy MMIO |
|---|---|---|
| Queue size | fixed by the device; the driver must have the memory | chosen by the driver, up to the device's limit |
| Page size and ring alignment | 4096, by definition | the driver says (`GUEST_PAGE_SIZE`, `QUEUE_ALIGN`) |
| Interrupt | chosen (MSI-X vector) | one line; must be acknowledged |

Since the driver chooses the size here, it takes the largest that fits the 32 KiB it sets aside:

```rust
pub fn queue_size(max: u32, room: usize) -> Option<u16> {
    let mut size = max.min(32768).checked_ilog2().map(|log| 1u32 << log)?;
    while size >= 2 {
        if super::Layout::legacy(size as u16).total <= room { return Some(size as u16); }
        size /= 2;
    }
    None
}
```

`checked_ilog2` is "the position of the highest set bit, or `None` for zero"; `1 << that` rounds down to a power of two. The `?` after `map` returns `None` from the whole function if there was no queue. QEMU offers 1024 and a queue of 1024 is exactly 32768 bytes - a coincidence the host test records.

The driver also checks the magic number and device kind itself before anything else, although the kernel did: it trusts what it reads, not what it was told.

## The test: `blkcheck`, and the file

The PC's disk is tested by using it - the file system lives on it. Here there is no file system on a disk yet, so the driver gets a client of its own: `blkcheck`, started by the kernel's self-test on the driver's two channels.

The boot test fills the disk with a pattern - byte `i` of sector `s` is `s·31 + i·7 + i/256` - so every sector differs from every other, and a sector read from the wrong place shows. `blkcheck` makes 348 checks:

- 153 single-sector reads (first, last, a run of 150), each compared with the pattern;
- 32 sectors at once through shared memory, and 3 at once - with the rest of the window checked to be untouched;
- past the end: refused, including a read that starts on the disk and ends off it;
- careless requests: empty, unknown, cut short, more sectors than the lent memory holds, a channel lent in place of memory;
- writes: refused on the read-only channel; one sector and 32 at once on the other, read back the other way round; the neighbours unchanged; the very last sector.

Then the part I care about most. A driver that kept written data in memory and never sent it to the device would pass every read-back. So after the boot, a script reads **the image file**:

```
the disk image, read from outside: 34 sectors written, 6109 as they were
```

Exactly sectors 100-132 and 6142 must hold the written pattern and every other sector the original.

The self-test runs driver and client **twice with the same handles**. The second driver finds the device as the first left it - a queue configured, perhaps an interrupt pending - and must reset it and start over. That is what restarting a crashed driver will be, tested before there is anything to restart it.

## Design decisions

| Decision | Chosen | Rejected, and why |
|---|---|---|
| Who quiets a raised line | the kernel switches it off at the controller until the driver waits again | the kernel acknowledges the device: it would have to know virtio. A separate "re-enable" system call: a driver could forget it |
| Edge or level | what the tree says | always level: contradicts the machine's description; always edge: wrong on a machine that says level |
| Where the re-enable happens | inside `interrupt_wait` | on return to user mode: the driver has not touched the device yet |
| One driver or two | one `blk` with a `Transport` | a second program `blk-mmio`: the queue and service code twice |
| Telling the descriptions apart | by length | a tag byte: the PC's message format would change for nothing |
| Queue size | the largest that fits the driver's memory | a fixed 128: works, and wastes what the device offers |
| How the driver is tested | a dedicated client, plus the image read from outside | read-back only: cannot tell a disk from a cache |
| Who starts the driver, for now | the kernel's self-test | `init`: it needs a keyboard, a registry and a file system first - the next steps |
| The modern interface (version 2) | found, reported "no driver speaks it yet" | written now: a second queue set-up before the first is used by anything |

## Bugs

The driver read its first sector on the first boot, on both controller versions. I did not expect that and spent the time on trying to break it instead (below).

One test failed when I ran all the AArch64 boots, and not a new one: the boot with 6 GiB and reserved memory expects the free memory within a few MiB of the total, and found 6123 MiB where 6124 was the least allowed. The kernel image had crossed a megabyte boundary - with only two small programs added.

That was the clue. `blk` for AArch64 was 82 KB; for the PC, 21 KB. `readelf -lW` showed why:

```
LOAD  0x010000 0x0000000000400000 … R E 0x10000
```

Every segment lined up on 64 KiB in the file: the padding entry 57 found in the C programs and fixed in *their* link line. The Rust programs have their own link flags, and I never looked. Fourteen programs, about 60 KB of zeroes each, all embedded in the kernel. One flag in `user/.cargo/config.toml` (`-zmax-page-size=4096`):

| | before | after |
|---|---|---|
| `blk` (AArch64) | 82 272 bytes | 20 832 |
| the kernel image | 1 664 280 bytes | 804 120 |

and the old test passes unchanged. I had first widened the test's window, which would have hidden this; I put it back.

## How I checked it

**Automated** (`make test64`, ending `test64: all passed`):

- Host: kernel library 171 tests (2 new: where an interrupt's trigger bits are, how a CPU's name becomes a route); `abi` 7 (1 new: the description through bytes, and neither kind accepting the other); `virtio` 8 (1 new: queue size).
- Boots - four with a driven disk, each with its own image, checked from outside afterwards:

| Boot | Machine | Must happen |
|---|---|---|
| `disk` | GIC v2, a second device beside the disk | 348 checks twice; 34 sectors written, the rest as they were |
| `disk-v3` | GIC v3, entered at exception level 2 | the same |
| `disk-ro` | the disk read-only | 340 checks twice, every write refused; image untouched |
| `disk-level` | a tree edited to say the line is level-triggered | the same as `disk` |
| `modern` | the newer interface | "no driver speaks it yet"; no driver runs; image untouched |

- Every other AArch64 boot must say the disk test was not run, and no `[blk]` line may appear.
- The self-test asserts that both programs exit 0, no frame and no channel is left after the second run, and that the device interrupted at least 20 times per run (it was 338 over the two) - so the driver was waiting, not asking in a loop.

**By hand** - twelve deliberate changes, each booted with a disk:

| Change | Result |
|---|---|
| driver never writes `INTERRUPT_ACK` | caught: a read is never answered, the boot does not finish |
| line never switched on again | caught: the same |
| queue address off by one page | caught |
| page size never told | caught |
| queue size never told | caught |
| last sector treated as out of range | caught: `the last sector can be read through a window` |
| registers handle maps the page before | caught: `these are not a virtio block device's registers` |
| tree read as level, switching-off kept | passes - as it should |
| tree read as level, line **not** switched off | caught: interrupt storm, the log stops mid-line |
| line not switched off (tree says edge) | **survives** - with an edge line nothing needs it; this is why `disk-level` exists |
| `QUEUE_ALIGN` never written | **survives**: QEMU's default is the value I write |
| v3 route not written | **survives**: the register starts as "CPU 0", the only CPU |

**Not tested:**
- The route on a machine where the right CPU is not CPU 0, and `QUEUE_ALIGN` on a device with another default: QEMU cannot show either wrong.
- A driver that crashes while holding the device - only the clean hand-over between two drivers.
- A disk error reported by the device (`IO_ERROR`).
- The polled path (a device with no interrupt in the tree) on AArch64.
- Two disks driven at once; a second one is found and ignored.
- The PC's `blk` is the same file with its port code now behind the `Transport`; it is covered by the existing PC boots, which passed, not by anything new.

## What is still missing

- **Isolation of registers.** The thirty-two places share four pages, eight to a page. The disk's driver is handed the whole page and could write to seven other devices' registers. Real boards give each device its own pages; for this machine the honest fix is the kernel mediating register access, or accepting it.
- **DMA is unconfined**, as on the PC: the device writes wherever the driver points it. Needs an IOMMU.
- **A file system on this disk**, then a keyboard, `init`, the shell - the rest of step 3g.
- The modern (version 2) interface, and devices other than block.
- The interrupt line stays allocated for ever; there are 16.

## What I learned

- **A message and a wire are different kinds of interrupt.** A message is an event; a wire is a state. A state has to be cleared by someone who understands the device - and in a microkernel that someone is not in the handler.
- **The kernel can keep order without understanding.** "Off until the driver asks again" needs no knowledge of what the device wants.
- **Make the safe thing the only thing.** Re-enabling inside `interrupt_wait` means there is no call to forget.
- **Acknowledge, then look.** The order of two harmless-looking lines decides whether a completion can be lost.
- **Code that the test machine cannot exercise needs a different machine** - here, the same one with an edited description.
- **Reading back is not proof of writing.** Only the file is the disk.
- **A test that starts failing for a dull reason may be pointing at something.** "The image got bigger" was true, and the wrong place to stop.
- **A good seam is found, not designed.** `blk` split along `Transport` cleanly because virtio itself separates the queue from how the registers are reached.

## Testing note

**1. A disk, driven by a program**

```
python3 kernel/kernel64/scripts/disk-pattern.py make /tmp/taaj-a64-disk.img 6143
make run-aarch64 RUN_A64="-drive file=/tmp/taaj-a64-disk.img,if=none,format=raw,id=d0 -device virtio-blk-device,drive=d0"
```

Nothing to type. To leave QEMU: **Ctrl+A, then X**. Near the end:

| Look for | Meaning |
|---|---|
| `[virtio] 0xa003e00 block device: registers in the page at 0xa003000, 64 KiB DMA at 0x…, interrupt 79, off until its driver waits - ready for a driver` | the kernel made the three capabilities |
| `[blk] virtio disk at 0xa003e00: 6143 sectors (3071 KiB), queue of 1024, interrupts` | the driver, a program, set the device up |
| `[blkcheck] 348 checks, all passed: 6143 sectors, 33 written at 100 and the last one` (twice) | every request and refusal behaved |
| `[process] … "blk" exited with code 0` (twice) | the driver ended when its clients left |
| `[selftest] disk: blk and blkcheck, twice on one device - every check passed; 338 interrupts passed to the driver; nothing leaked` | interrupts were used; no memory or channel left behind (the number may differ a little) |

Then, after leaving QEMU, the disk itself:

```
python3 kernel/kernel64/scripts/disk-pattern.py check /tmp/taaj-a64-disk.img 6143 written
```

should print `the disk image, read from outside: 34 sectors written, 6109 as they were`.

**2. The other interrupt controller**

```
python3 kernel/kernel64/scripts/disk-pattern.py make /tmp/taaj-a64-disk.img 6143
make run-aarch64 RUN_A64="-M virt,gic-version=3 -drive file=/tmp/taaj-a64-disk.img,if=none,format=raw,id=d0 -device virtio-blk-device,drive=d0"
```

| Look for | Meaning |
|---|---|
| `[irq] GIC version 3: 288 interrupt lines, all off but the timer's` | version 3 |
| the same `[blk]`, `[blkcheck]` and `[selftest] disk:` lines as in 1 | the interrupt was routed there too |

**3. A read-only disk**

```
python3 kernel/kernel64/scripts/disk-pattern.py make /tmp/taaj-a64-disk.img 6143
make run-aarch64 RUN_A64="-drive file=/tmp/taaj-a64-disk.img,if=none,format=raw,id=d0,readonly=on -device virtio-blk-device,drive=d0"
python3 kernel/kernel64/scripts/disk-pattern.py check /tmp/taaj-a64-disk.img 6143 untouched
```

| Look for | Meaning |
|---|---|
| `[blk] virtio disk at 0xa003e00: 6143 sectors (3071 KiB), read-only, queue of 1024, interrupts` | the driver noticed |
| `[blkcheck] 340 checks, all passed: 6143 sectors, read-only - every write refused` | no write got through |
| `the disk image, read from outside: 0 sectors written, 6143 as they were` | and the file agrees |

**4. No disk, and a disk with no driver**

```
make run-aarch64
make run-aarch64 RUN_A64="-global virtio-mmio.force-legacy=false -drive file=/tmp/taaj-a64-disk.img,if=none,format=raw,id=d0 -device virtio-blk-device,drive=d0"
```

| Look for | Meaning |
|---|---|
| `[selftest] disk: no block device a driver exists for - not run` | both |
| `[virtio] 0xa003e00 block device: the modern interface - no driver speaks it yet` | the second only |

**5. Host checks**

```
cd kernel/kernel64 && cargo test --lib --target x86_64-unknown-linux-gnu gic
cd kernel/abi && cargo test
cd virtio && cargo test
```

Each should end `test result: ok.` - 12, 7 and 8 tests.

**6. The PC's disk, through the same driver**

```
make run64
```

| Type | You should see |
|---|---|
| `ls` | the files on the disk, as before |
| `disk` | `disk: … sectors of 512 bytes (… KiB)`, as before |

Automated: `make test64`
