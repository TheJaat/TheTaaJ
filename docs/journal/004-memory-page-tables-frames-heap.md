# Memory: Page Tables, a Frame Allocator and a Heap

*Journal entry #4 · and testing a kernel in three layers*

After entry #3 the kernel could report its own crashes. But it was still living in the bootloader's house: the loader's page tables, no way to allocate memory, no `Vec`, no `Box`. This entry is where the kernel takes ownership of memory — and where I settled how this kernel is tested, which shaped everything after.

---

## Three things called "memory management"

1. **Physical memory**: which 4 KiB pages ("frames") of RAM are free? → the *frame allocator*.
2. **Virtual memory**: which virtual address leads to which frame, and with what permissions? → *page tables*.
3. **Small allocations**: `Box`, `Vec`, `String` want 24 bytes here, 300 there. → the *heap*.

Each builds on the one before.

## Addresses as types

A physical address and a virtual address are both 64-bit numbers. In C both would be `uint64_t`, and passing one where the other was meant compiles fine and corrupts memory at run time. In Rust I wrapped each in its own type:

```rust
#[repr(transparent)]
pub struct PhysAddr(u64);
#[repr(transparent)]
pub struct VirtAddr(u64);
```

Mixing them is now a *compile error*, at zero cost: `repr(transparent)` means the struct is laid out exactly like the `u64` inside it. This is the "newtype" pattern, and it caught real mistakes while I wrote the code below.

## The frame allocator: a bitmap

One bit per frame: 1 = in use, 0 = free. For 512 MiB of RAM that is 131,072 bits — 16 KiB.

The decision that matters is the starting state: **everything starts "in use"**. The kernel then frees the ranges the memory map says are usable, and re-reserves what it knows is taken (itself, the loader's low memory). Starting from "used" means a mistake in reading the map can only *waste* memory — never hand out RAM that is not there, or that something else occupies.

Allocation returns the lowest free frame. Predictable, which matters in tests.

## Page tables

A virtual address is split into four 9-bit indices and a 12-bit offset. Each index selects one of 512 entries in a table that is itself one 4 KiB page:

```
bits 47-39  PML4 index  -> a PDPT
bits 38-30  PDPT index  -> a PD      (or a 1 GiB page)
bits 29-21  PD   index  -> a PT      (or a 2 MiB page)
bits 20-12  PT   index  -> a 4 KiB page
bits 11-0   offset within the page
```

The register `CR3` holds the physical address of the top table. The tables contain *physical* addresses — the CPU walks them without translation. But the kernel can only use *virtual* addresses. So to edit a page table, the kernel needs some mapping through which it can reach any physical frame.

### The direct map

The answer is to map **all of physical memory** at one fixed offset: physical address `p` is always readable at virtual `0xFFFF_8000_0000_0000 + p`. With that, reaching any page table — or any frame — is one addition.

The kernel's address space, as built in this step (and extended later):

```
0x0000_0000_0000_0000  lower half: nothing (for user space).
                       Address 0 faults, so null pointers do.
0xFFFF_8000_0000_0000  the direct map: all physical memory,
                       read/write, never executable
0xFFFF_C000_0000_0000  the kernel heap, growing upward
0xFFFF_FFFF_8000_0000  the kernel image: code read+execute,
                       rodata read-only, data read/write -
                       and an unmapped guard page below the
                       boot stack
```

### Permissions, enforced by hardware

The kernel maps *itself* with the tightest permissions each part allows:

- **code**: readable and executable, not writable;
- **read-only data**: readable only;
- **data**: readable and writable, **not executable**.

Then it turns on the CPU features that make those bits real: `EFER.NXE` (the no-execute bit), and `CR0.WP` — without which ring 0 could write to read-only pages anyway.

Why bother, in my own kernel? Because a stray write that lands in kernel code is otherwise silent corruption that shows up as nonsense minutes later. With these permissions it is an immediate page fault naming the exact address. And "memory is never both writable and executable" (W^X) closes the most common route by which a memory bug becomes code execution.

The boot stack also gets an **unmapped guard page** below it. Overflowing the stack hits it and faults at once, and the exception handler recognises the address and says "kernel stack overflow" instead of a mysterious page fault.

## The heap

Rust's `Box`, `Vec` and `String` need one thing: a type that implements `GlobalAlloc`, registered with `#[global_allocator]`. Mine is a **linked-list allocator**:

- Free memory is a list of blocks. Each free block stores its own size and a pointer to the next one *inside the free memory itself* — so the list costs nothing beyond the memory it describes.
- **Allocate**: walk the list, take the first block big enough (first fit), give back what is left over.
- **Free**: put the block back in address order, and **merge** it with its neighbours if they touch. Without merging, the heap decays into many small blocks that no larger request fits.

Everything is in 16-byte units, so a leftover is either zero or big enough to be a block — no unusable slivers.

When the heap runs out, the kernel maps fresh frames at the end of the heap area and hands them to the allocator. It grows on demand.

It is O(n) and simple. A faster allocator for common fixed sizes can sit on top later. I wanted the first one to be something I could hold entirely in my head.

## Testing in three layers

This is the part I am most glad I did early.

Most of the code above is **pure logic**: given some memory, do the right bookkeeping. It does not need a real CPU. So the kernel is split into two crates in one directory:

- **`lib.rs` (`kernel64::`)** — addresses, the frame bitmap, page tables, the heap, locks. No hardware access. It compiles for the kernel *and* for my development machine.
- **`main.rs`** — the kernel proper, which wires that logic to real RAM (`mm.rs`), loads `CR3`, flushes the TLB.

The page table code, for example, is generic over where frames come from and reads tables through an offset — so on the host, a `Vec<u8>` stands in for physical RAM.

That gives three layers of test:

1. **Host unit tests** (`make kernel64-test`). Milliseconds, with a debugger. They include a 20,000-step randomised stress test of the heap — random allocations and frees, checking after every step that nothing overlaps and everything merges back — and a lock test with real threads contending.
2. **Self-tests at every boot** (`selftest.rs`). The host tests prove the algorithms; these prove the *wiring* on the real machine: allocate and free frames, map a page and read it through the direct map, check the kernel's own permissions, grow the heap and check it all comes back.
3. **Fault boots** (`make fault64-test`). One build per deliberate fault: an unmapped read, a double fault, a write to read-only data, executing data, a stack overflow. Each must be caught and named.

`make test64` runs all three.

## What I learned

- **Split logic from hardware and most of a kernel becomes ordinary, testable code.** A wrong decision in a host test is an assertion failure with a line number. The same bug on the real machine is a hang.
- **Start from the safe state**: "everything in use" for frames, "nothing mapped" for the lower half, "not writable" for code.
- **Lock order is a design decision**: whenever more than one lock is held, it is heap, then page tables, then frames. One fixed order makes deadlock between them impossible.

---

## Testing note

```
make kernel64-test     # the host unit tests
make fault64-test      # one boot per deliberate fault
make boot64-test       # a normal boot, with the self-tests
```

In a normal boot's log (`make run64`, in the terminal), these lines should appear:

| Line | Meaning |
|---|---|
| `[selftest] frames: allocate, free, reuse` | The frame allocator works on real RAM |
| `[selftest] paging: map, alias through the direct map, unmap` | A page mapped at one address is the same memory seen through the direct map |
| `[selftest] protections: code RX, rodata R, data RW+NX, null and stack guard unmapped` | The kernel's own mappings have the intended permissions |
| `[selftest] heap: Box, Vec, String; grew from N KiB to M KiB` | The heap works and grows on demand |
| `[selftest] memory: all passed` | All four |

In the OS, the `mem` command shows frames and heap in use.
