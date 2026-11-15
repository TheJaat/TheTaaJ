# The Disk: PCI, MSI-X, DMA Memory and a virtio Block Driver in User Space

*Journal entry #14 · the first storage device, driven from ring 3*

Files in memory vanish at power-off. This entry reaches a real disk — and it is the hardest driver so far, because a disk controller does something a keyboard never did: it **reads and writes memory by itself**. The kernel's job is to find the device and turn each thing the driver needs into a capability. The driver, `blk`, is an ordinary supervised process.

---

## Which disk?

Real machines have AHCI or NVMe controllers. Virtual machines also offer **virtio**: a standard interface designed for virtual devices, with a published specification, offered by QEMU, KVM and cloud hypervisors. It is far simpler than AHCI and teaches every concept a real disk driver needs — device discovery, shared rings, DMA, interrupts. AHCI and NVMe can follow as further drivers *behind the same protocol*; nothing above the driver will change.

## Finding devices: PCI

Devices on a PC sit on the **PCI bus**. Each one has a *configuration space* — a small table of registers — addressed by bus, device and function number, and reached through two I/O ports (`0xCF8` to choose, `0xCFC` to read or write).

The kernel scans it. For each device it reads:

- **vendor and device id** — who made it, what it is (`1af4:1001` is a virtio block device);
- **class** — storage, display, network, bridge;
- **BARs** (Base Address Registers) — where the device's own registers live: a range of I/O ports or of memory. Their *size* is found by a curious ritual: write all ones, read back, and the bits that stayed zero give the size;
- **capabilities** — a linked list of optional features, MSI-X among them.

All the logic — scanning, BAR sizing, walking the capability list — is in the hardware-free library, generic over "something that reads and writes configuration space". On the host it is tested against a **simulated bus**. Only the two-port access is real hardware code.

The capability list is a linked list supplied by a device, so it is walked with a bound: a device (or a bug) presenting a loop must not hang the kernel.

## What the driver needs — three capabilities

### 1. Its I/O ports

The virtio device's registers are a range of I/O ports far above the PS/2 ports. The port capability from entry #10 was extended to cover all 65,536 ports — and the trick of storing only the bytes up to a process's highest port keeps context switches cheap for everyone else.

### 2. DMA memory

The driver and device share a ring of requests **in memory**. The driver tells the device the *physical* address of that memory, and the device reads and writes it directly — **DMA**, direct memory access — with no CPU involvement.

That needs memory with two special properties:

- **physically contiguous**: the device is given one address and a length, so the frames must be adjacent. The frame allocator gained `allocate_contiguous`;
- **a known physical address**: normally a process has no idea where its pages physically are, and should not. For DMA it must.

So the kernel creates a memory object of contiguous frames and tells init its physical address along with the handle.

### 3. Its interrupt: MSI-X

The old way to interrupt is a physical pin routed through the I/O APIC (entry #6): shared between devices, level-triggered, needing ACPI to find out which pin. The modern way is **MSI-X**: the device raises an interrupt by *writing a message* to a special address, and the message names the vector directly.

No routing tables, no sharing, no ACPI. The kernel programs the device's MSI-X table to raise a vector reserved for devices (`0x30`–`0x3F`), and wraps it in the same interrupt object as the keyboard's. To the driver it is just a handle to wait on.

## Handing it over

`devices.rs` packs these into a message for init: the device's description, its ports, its DMA memory, its interrupt.

```
[pci] 00:04.0 virtio block device: ports 0xc000+0x80, 64 KiB DMA at 0x922000, MSI-X - handed to init
```

init reads device messages until the kernel closes its end of that channel — so it knows when it has heard about everything — and starts `blk` only if there is a disk. If there is none, init **closes** the shell's disk channel, so `disk` answers "no disk" at once instead of waiting forever for a driver that will never exist.

## The virtqueue

A virtio device and its driver share a **virtqueue**, three structures in the DMA memory:

- a **descriptor table**: each entry names a piece of memory (address, length, readable or writable by the device), and may chain to another;
- the **available ring**: the driver puts descriptor numbers here — "work for you";
- the **used ring**: the device puts them back here — "done".

A read of one sector is a chain of three descriptors: a header (*read, sector N*), a 512-byte buffer the device writes into, and a status byte the device writes. The driver adds the chain to the available ring and *notifies* the device (one port write); the device does the work, puts the chain in the used ring, and interrupts.

The device registers are only for setup and for saying "there is something new". **The data never goes through them.**

The `virtio` crate holds this — register map, free-descriptor list, both rings with their wrapping indexes, the block request format — as pure logic over a pointer to the queue's memory, tested on the host against a **simulated disk** that plays the device's part.

One subtlety: the driver and the device are, in effect, two threads sharing memory without a lock. The order of writes matters — fill the descriptor, *then* publish its index — and the code places memory fences so neither the compiler nor the CPU reorders them.

## The driver

`blk` resets the device, negotiates features (it needs none, but it must notice "read-only"), sets up the queue in the DMA memory, and then serves requests from a channel (`abi::block`): `INFO`, `READ` and `WRITE` of one sector. While the device works, the driver **sleeps on the interrupt**.

Clients use `taaj::disk`, with a reply channel per request — the pattern from entry #13.

## An honest hole: no IOMMU

The driver chooses the physical addresses it puts in descriptors. Nothing stops a *malicious* driver from naming memory that is not its own — and the device would obediently read or write it. A user-mode driver with DMA is, today, **trusted not to do that**.

The hardware answer is an **IOMMU** (Intel VT-d, AMD-Vi): page tables for devices, limiting each to the memory its driver was given. Until TheTaaJ programs one, user-mode drivers protect against driver *bugs* — a crash, a wild pointer in the driver's own code — but not against a driver that sets out to attack. That is still most of the value, and it is written down rather than glossed over.

Also not yet: more than one request in flight at a time.

## How I checked it

The boot tests attach a fresh 4 MiB disk image and:

- read its size and its first sector;
- write text to a sector and read it back;
- ask for a sector past the end — refused;
- **after QEMU exits**, look in the image file itself for the text the OS wrote. The data really reached the disk, not just a cache in the driver.

## What I learned

- **A driver needs exactly three things** — registers, memory the device can reach, an interrupt — and each can be a capability.
- **DMA is the limit of software isolation.** Past this point, isolation needs hardware help.
- **Simulate the other side.** The virtqueue logic was debugged against a fake device on the host, where a mistake is a failed assertion. In QEMU the same mistake is a device that silently never answers.

---

## Testing note

```
make run64
```

In the terminal log:

| Line | Meaning |
|---|---|
| `[pci] 00:04.0 1af4:1001 class 01.00 SCSI storage` (among other `[pci]` lines) | The bus scan found the disk |
| `[pci] 00:04.0 virtio block device: ports 0x…, 64 KiB DMA at 0x…, MSI-X - handed to init` | Its three resources became capabilities |
| `[blk] virtio disk at 00:04.0: 32768 sectors (16384 KiB), queue of 256, MSI-X interrupts` | The ring-3 driver brought the device up (the persistent disk is 16 MiB) |

In the QEMU window:

| Type | You should see |
|---|---|
| `disk` | `disk: 32768 sectors of 512 bytes (16384 KiB)` |
| `disk read 0` | A hex dump beginning `4a 41 46 53` — the letters `JAFS`, the file system's signature |
| `disk read 99999` | `disk: sector 99999: no such sector` |
| `disk write 5 hello` | `disk: sector 5: the disk is read-only` — since entry #15 only the file service may write; the shell's view is read-only |
| `disk check` | `disk check: all 9 checks passed` (added in entry #18) |
| `ps` | `blk` among the services |

Automated: `make kernel64-test` (PCI against a simulated bus, the virtqueue against a simulated disk), `make boot64-test`, `make boot64-uefi-test`.
