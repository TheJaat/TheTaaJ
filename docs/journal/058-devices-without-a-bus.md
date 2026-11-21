# Devices without a bus

*Journal entry #58 · Era 3, step 3g, part 1. The AArch64 kernel finds its virtio devices. There is no bus to ask: the device tree names thirty-two places where one might be, and the kernel reads the first registers of each to learn which are occupied and by what. Finding only - a driver gets nothing yet.*

```
[virtio] 0xa003c00: source of random numbers (legacy interface), interrupt 78 (edge)
[virtio] 0xa003e00: block device (legacy interface), 6143 sectors (3071 KiB), interrupt 79 (edge)
[virtio] 32 places at 0xa000000 - 0xa004000: 2 with a device, 30 empty
```

---

## Why

After entries 56 and 57 the AArch64 kernel runs every program that needs nothing but a CPU. The rest - `init`, the shell, the file service on a real disk - need devices. On the PC the disk came in entry 14, and it began with a scan: ask the PCI bus what is on it. I sat down to do the same here and found there was nothing to ask.

## Two ways to learn what a machine has

| | PC (entry 14) | QEMU's AArch64 `virt` machine |
|---|---|---|
| Where devices are | on a bus with a standard way to list it (PCI configuration space) | at fixed addresses the board's designer chose |
| How the kernel learns | asks the bus: "who is at 0:4.0?" | is told, by the device tree |
| What it is told | vendor and device id - *what* the device is | an address, a size, an interrupt - *where* something may be |
| Addresses and interrupts | assigned at boot (BARs), movable | fixed for ever |
| virtio's registers | I/O ports, in BAR 0 | 512 bytes of memory ("virtio-mmio") |

The tree's entry for one device looks like this (decoded from what QEMU passes):

```
virtio_mmio@a003e00 {
    compatible = "virtio,mmio";
    reg = <0x0 0xa003e00 0x0 0x200>;
    interrupts = <0x0 0x2f 0x1>;
    dma-coherent;
};
```

What surprised me: there are **thirty-two** of these, always. I booted with no devices at all and the tree still listed all thirty-two. The tree is written before QEMU knows which will be used, so it describes *places*, like numbered sockets on a wall. Whether anything is plugged into one, the socket has to say.

## What a place says about itself

The first four registers of every virtio-mmio place, each 32 bits:

```
0x000  magic       0x74726976 - the letters "virt" - or these are not virtio registers
0x004  version     1: the legacy interface    2: the modern one
0x008  device id   0: nothing here   1: network   2: block   3: console   4: random numbers ...
0x00c  vendor id
```

So the "bus scan" is: for each place, read four numbers.

I put that in the `virtio` crate, next to the PCI register names, as a function that takes *a way to read a register* instead of an address:

```rust
pub fn identify(read: impl Fn(u32) -> u32) -> Result<Identity, Nothing> {
    let magic = read(MAGIC);
    if magic != MAGIC_VALUE {
        return Err(Nothing::NotVirtio { magic });
    }
    let version = read(VERSION);
    if version != 1 && version != 2 {
        return Err(Nothing::UnknownVersion { version });
    }
    let device = read(DEVICE_ID);
    if device == 0 {
        return Err(Nothing::Empty);
    }
    Ok(Identity { version, device, vendor: read(VENDOR_ID) })
}
```

`impl Fn(u32) -> u32` means "any function or closure from a `u32` to a `u32`". In the kernel that closure does a volatile read of device memory. In the host tests it indexes an array. The logic is the same code in both, which is the reason for the shape: the part that can be wrong (the order of checks, what counts as empty) runs on my desk, and the part that cannot be tested there is one line.

The order is the point. Nothing but the magic number may be believed until the magic number is right. One test hands `identify` a reader that panics if asked for anything past register 0, with a wrong magic in it.

The result is a `Result` with a meaningful error type: `Empty` is the normal case and is only counted; `NotVirtio` and `UnknownVersion` mean the tree and the machine disagree, and are printed.

## From the tree to the registers

Three pieces, in order:

**1. The tree reader** (`devicetree.rs`) gained `DeviceSlot { registers, interrupt, edge }` and collects every enabled node compatible with `"virtio,mmio"`.

The `interrupts` property is three numbers, and I had already decoded this once, for the timer, inline. Now there are two users, so it is a function:

```rust
pub fn gic_interrupt(kind: u32, number: u32, flags: u32) -> (u32, bool) {
    (number + if kind == 1 { 16 } else { 32 }, flags & 0b11 != 0)
}
```

`<0 0x2f 1>` is "shared, number 47, rising edge": interrupt 32 + 47 = **79** in the controller's own numbering, edge-triggered. The 32 is because numbers 0-15 are interrupts CPUs send each other and 16-31 are private to each CPU (the timer's 27 is one of those); shared device interrupts start after.

**2. The kernel maps the registers.** All thirty-two places are side by side - 0x200 bytes each, 0xa000000 to 0xa004000, four pages - so `mmio_devices::find` maps the whole stretch once with `mm::map_mmio` (device memory: uncached, every access really happens, in order) instead of thirty-two times.

**3. It reads each place** through `identify`, and for a block device also reads its size. A virtio block device's own configuration starts at offset 0x100 with its capacity in 512-byte sectors, and that may be read before any driver has introduced itself. It makes the log line checkable: I made the test disk 3 MiB less one sector, and the kernel says `6143 sectors (3071 KiB)`. No default would produce that number.

## What I noticed in the output

- **QEMU fills the places from the last one down.** One disk lands at `0xa003e00`, place 31; the second device at `0xa003c00`. A kernel that assumed "the first place" would find nothing.
- **Legacy by default.** QEMU's virtio-mmio devices speak version 1 unless told otherwise (`-global virtio-mmio.force-legacy=false`). Version 1 lays its queue out exactly as legacy PCI does, which is what my `virtio::Queue` already implements - so the driver next step can start there. The kernel reports both versions.
- **The tree says "edge".** The device actually holds its line up until the driver acknowledges, which is how a level interrupt behaves. Which of the two the kernel should believe matters for the next step, not this one; for now it prints what the tree says.

## Design decisions

| Decision | Chosen | Rejected, and why |
|---|---|---|
| Where `identify` lives | the `virtio` crate, taking a reader closure | the kernel, reading pointers: untestable off the machine |
| What the tree reader records | every place, occupied or not | only occupied ones: the tree cannot know; probing needs the MMU and belongs to the kernel proper |
| Mapping | one stretch for all places | one mapping per place: thirty-two pieces of address space for four pages |
| A place that is not virtio | reported, and skipped | a panic: a wrong tree should cost one device, not the boot |
| Block size at discovery | read and printed | left to the driver: then nothing in the log would prove the right registers were read |
| The dependency on `virtio` | moved from "PC only" to shared in `Cargo.toml` | a second copy of the constants in the kernel |
| Scope of this entry | finding only | finding plus hand-over plus a driver: three new things at once, and nothing to bisect when it fails |

## Bugs

None found. The kernel's first boot with a disk attached printed the lines at the top.

One thing I had to get right in the order of work, and which the structure now reflects: the tree is read early, at its physical address, before the kernel's own page tables exist; the devices can only be read after, through a mapping made for device memory. So "which places exist" and "what is in them" are two steps with the memory manager in between, and the tree reader never touches a device.

## How I checked it

**Automated** (`make test64`, ending `test64: all passed`):

- Host, `virtio` crate: 7 tests (2 new) - each kind of answer from `identify`, and that nothing past the magic number is read when it is wrong.
- Host, kernel library: 169 tests (2 new, 1 extended) - `gic_interrupt` for the timer, a virtio slot and the serial port; slots with and without an interrupt, disabled, and without `reg`; and QEMU's real tree must give exactly 32 slots of 0x200 bytes with interrupts 48 to 79, all edge.
- Boots: every ordinary AArch64 boot must print the summary line with the number of devices it was given (0 for all the old ones) and exactly that many device lines. Two new boots: `disk` (a 6143-sector disk and a random-number device; both lines checked, with place, size and interrupt) and `modern` (the same disk with the newer interface).

**By hand** - three breakages with a disk attached, each of which the `disk` boot's patterns would fail:

| Change | What the kernel then printed |
|---|---|
| size read from offset 0x108 | `0 sectors (0 KiB)` |
| wrong address arithmetic for a place | sixteen `not a virtio device (it begins 0x00000000…)` lines, 0 devices |
| shared interrupts numbered from 16 | `interrupt 63` |

**Not tested:** a machine whose tree lists more than 32 places or spreads them over more than 1 MiB (the code refuses and says so; no boot does that); a place whose magic is wrong on a real boot (host test only); any device actually being *used*.

## What is still missing

- **Everything after finding.** A driver needs the registers mapped into its own address space, memory the device can write to (DMA), and its interrupt - each as a capability. That is the next step.
- **Shared interrupts.** The kernel's interrupt controller code enables exactly one line, the timer's. Device lines need enabling, a trigger mode, and a way to keep a line quiet until its driver has dealt with it.
- **A block driver that speaks memory-mapped registers.** `blk` uses I/O ports.
- The other things in the tree: a real-time clock (`pl031`), flash, a PCI host bridge (`pci-host-ecam-generic`) - QEMU's `virt` has a PCI bus as well, which I am ignoring for now.

## What I learned

- **A bus is a way to ask; a tree is being told.** PCI answers "what are you?". The device tree answers "where might something be?" - and for virtio the second question still needs the first asked, of the device itself.
- **A description can be of sockets, not of plugs.** Thirty-two entries, zero devices.
- **Pass the reader, not the address.** A function that takes "how to read a register" can be tested with an array.
- **Make test data improbable.** A disk of 6143 sectors proves the right register was read; a disk of 2048 could be a coincidence or a default.
- **Check the magic number first, and prove that you do.**

## Testing note

**1. No devices: thirty-two empty places**

```
make run-aarch64
```

Nothing to type. To leave QEMU: **Ctrl+A, then X**.

| Look for | Meaning |
|---|---|
| `[virtio] 32 places at 0xa000000 - 0xa004000: 0 with a device, 32 empty` | every place the tree lists was looked at; none is occupied |

**2. A disk and a second device**

```
head -c 3145216 /dev/zero > /tmp/taaj-a64-disk.img
make run-aarch64 RUN_A64="-drive file=/tmp/taaj-a64-disk.img,if=none,format=raw,id=d0 -device virtio-blk-device,drive=d0 -device virtio-rng-device"
```

| Look for | Meaning |
|---|---|
| `[virtio] 0xa003e00: block device (legacy interface), 6143 sectors (3071 KiB), interrupt 79 (edge)` | the disk, in the last place, with its real size |
| `[virtio] 0xa003c00: source of random numbers (legacy interface), interrupt 78 (edge)` | the second device, one place down |
| `[virtio] 32 places at 0xa000000 - 0xa004000: 2 with a device, 30 empty` | the count |

**3. The newer interface**

```
make run-aarch64 RUN_A64="-global virtio-mmio.force-legacy=false -drive file=/tmp/taaj-a64-disk.img,if=none,format=raw,id=d0 -device virtio-blk-device,drive=d0"
```

| Look for | Meaning |
|---|---|
| `[virtio] 0xa003e00: block device (modern interface), 6143 sectors (3071 KiB), interrupt 79 (edge)` | the same disk, version 2 of the registers |

**4. What the tree itself says** (needs `dtc`: `sudo apt install device-tree-compiler`)

```
qemu-system-aarch64 -M virt -cpu cortex-a72 -machine dumpdtb=/tmp/virt.dtb
dtc -I dtb -O dts /tmp/virt.dtb | grep -c 'compatible = "virtio,mmio"'
```

should print `32`.

**5. Host checks**

```
cd virtio && cargo test
cd kernel/kernel64 && cargo test --lib --target x86_64-unknown-linux-gnu devicetree
```

Both should end `test result: ok.` - 7 tests and 26 tests.

**6. The PC is unchanged**

```
make run64
```

| Type | You should see |
|---|---|
| `ls` | the files on the disk, as before |

Automated: `make test64`
