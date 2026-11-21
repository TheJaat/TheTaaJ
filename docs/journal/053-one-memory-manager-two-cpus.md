# One Memory Manager, Two CPUs

*Journal entry #53 · Era 3, step 3c, second half. `mm.rs` - the frame allocator, the kernel's own page tables, the direct map, the heap - loses its "x86_64 only" mark. The AArch64 kernel leaves the rough tables it started on for real ones, turns off the low half of its address space, and passes the same memory self-test as the PC.*

```
[memory] 130737 frames free of 393216 (510 MiB), bitmap 48 KiB at 0x40000000
[memory] own page tables at 0x4000c000: direct map 0xffff800000000000, NX on, write protection on
[memory] heap at 0xffffc00000000000, 1024 KiB to start
[tree] read again through the direct map: the same 56 nodes, in memory kept out of the allocator
[selftest] frames: allocate, free, reuse; the kernel's own are not free
[selftest] top of memory: the highest free frame, at 0x5ffff000, written and read back
[selftest] paging: map, alias through the direct map, unmap
[selftest] protections: code RX, rodata R, data RW+NX, null and stack guard unmapped
[selftest] heap: Box, Vec, String; grew from 1024 KiB to 6144 KiB
[selftest] memory: all passed
[paging] the kernel's own tables, read by the kernel and by the CPU: 8 lookups, all the same
[paging] the CPU refuses a write to code and to constants, and allows one to variables and the heap
[paging] the low half is off: address 0 and 0x40080d9c, where this code really is, translate to nothing
[paging] one address mapped to the page at 0x40657000, then to the one at 0x40658000: the CPU reads the new page there
```

Every `[memory]` and `[selftest]` line there is printed by code that also runs on the PC. That is the point of the entry.

---

## Why

After entry 52 the AArch64 kernel could *write* page-table entries, and that was all. It still ran on the tables its first forty instructions of assembly made:

- the whole image in 2 MiB pieces, each **readable, writable and executable** - code that can be overwritten, data that can be run;
- no page under the stack to catch an overflow;
- its own physical addresses still mapped low, so a null pointer plus a large enough offset *read something*;
- no frame allocator, so no heap, so no `Box`, `Vec` or `String` - and without those, none of the rest of the kernel (threads, IPC, processes) can even be compiled in.

Everything after this step needs memory. Interrupts need a mapped controller; threads need stacks; programs need address spaces.

The PC kernel has had all of this since entry 4, in `mm.rs`. So the question was the same as last time: write it again for AArch64, or make the one there is serve both.

## What `mm.rs` assumed about being on a PC

I read `mm::init` looking for every place it knew which machine it was on. There were seven:

| # | What it assumed | Why that is PC-only |
|---|---|---|
| 1 | Memory is described by the loader's `BootInfo` | AArch64 gets a device tree |
| 2 | The kernel's bytes are at `virtual - KERNEL_WINDOW` | A PC loader always puts the image at 1 MiB. An AArch64 machine has memory wherever it likes, and the image wherever the loader likes |
| 3 | The first megabyte is special, and one page of it is kept for starting CPUs | Real mode exists only on x86 |
| 4 | Before the switch, the first 4 GiB can be read and written at their physical addresses | That is what *my* PC loader maps. The AArch64 start code maps one GiB |
| 5 | "Frames come lowest first, so the early tables are below 4 GiB" | True, but only as a comment. Nothing enforced it |
| 6 | `mmu::enable_protection` then `mmu::activate` switches tables | Loading a new root is one instruction on x86. On AArch64 it has a rule (below) |
| 7 | A new mapping can be used the moment it is written | Not guaranteed on AArch64 |

None of the seven is in the *algorithm*. Placing a bitmap, freeing the usable ranges, building four levels of tables, growing a heap - all of that is the same. So: one `mm.rs`, and a short list of questions it asks `arch::mmu`.

## What the memory manager is given

`init` used to take the `BootInfo`. It now takes this:

```rust
pub struct BootMemory<'a> {
    /// What memory there is, and of what kind. Regions may overlap;
    /// where they do, anything not usable wins.
    pub regions: &'a [MemoryRegion],
    /// Physical ranges that hold something already: the boot info, the
    /// ramdisk, the device tree. Unused places are `0..0`.
    pub taken: [Range<u64>; 4],
    /// Physical ranges to put in the direct map besides the memory
    /// itself: the screen, where firmware keeps its tables.
    pub also_mapped: [Range<u64>; 2],
}
```

Each `kernel_main` fills it in from what its machine provides. The PC's is a few lines moved out of `init` (assumptions 1 and 3 go with them). AArch64's is new:

```rust
let usable   = machine.memory().iter().map(|r| MemoryRegion::new(r.start, r.size, MEMORY_USABLE));
let reserved = machine.reserved().iter().map(|r| MemoryRegion::new(r.start, r.size, MEMORY_RESERVED));
```

(`0..0` is an *empty range*: it starts and ends at the same place, so it overlaps nothing. A fixed array with empty places is how a list of "up to four things" is written when there is no heap yet to put a `Vec` in - and there is not, since this is the code that makes the heap.)

### Reserved memory in a device tree

A device tree can say "this memory exists but is not yours" in two places, and entry 51 read neither into `Machine`:

- a list in the blob's header area, outside the tree - pairs of (start, size);
- a `/reserved-memory` node, whose children each describe one range.

`Machine::read` now gathers both into `machine.reserved`. A child with no address (it asks for memory *to be found for it*) and a disabled child reserve nothing.

QEMU's trees reserve nothing at all, so to see this work on a booting kernel I needed a tree that does. `scripts/dtb-reserve.py` adds an entry to a blob's list. The boot tests use it twice (see "How I checked it").

## The questions asked of the CPU

```rust
mmu::kernel_physical(virt) -> u64        // where is this byte of my image, really?
mmu::early_reach() -> Range<u64>         // what physical memory can I touch before the switch?
mmu::supports_no_execute() -> bool
mmu::map_early_devices(tables, frames)   // carry over what was mapped before I existed
mmu::install_kernel_tables(root, nx)     // make these the tables
mmu::commit_mappings()                   // I have added mappings and am about to use them
mmu::flush(virt)                         // forget this page's translation
```

| | x86_64 | AArch64 |
|---|---|---|
| `kernel_physical` | `virt - KERNEL_WINDOW` | `virt - IMAGE_DELTA`, measured at start |
| `early_reach` | `0..4 GiB` | the one GiB the image was loaded in |
| `supports_no_execute` | asks CPUID | `true` |
| `map_early_devices` | nothing (the console is an I/O port) | the serial port's 2 MiB, again |
| `install_kernel_tables` | set NXE, WP, PGE; load CR3 | the routine below; then turn the low half off |
| `commit_mappings` | nothing | `dsb ishst; isb` |

### `commit_mappings`: a write the CPU has not seen yet

On x86, when the CPU finds no mapping for an address, it looks in the tables again before it faults. So a mapping written one instruction ago is seen.

AArch64 promises less. A write to a table may sit in the CPU's store buffer, and instructions already under way may have looked before it landed. Two instructions make it certain: `dsb` ("data synchronisation barrier") waits until the write has really happened, `isb` ("instruction synchronisation barrier") makes everything after it start afresh. Without them the first use of a new page *may* fault, rarely, depending on timing - the worst kind of bug.

`mm.rs` now calls `commit_mappings` after each batch of new mappings: when the heap grows, when a stack is mapped, when a device is. On the PC the function is empty and the compiler removes the call.

## Memory in two goes

Before the switch the kernel can only touch what `early_reach` covers. In that time it must write the frame bitmap and every page table. On the PC that was assumption 5: frames come lowest first, and low memory is in reach. On AArch64 with 6 GiB, the reach is 1 GiB and memory goes on for five more.

It would work by luck there too - the lowest frames are in the image's GiB on QEMU. But a real board's loader may put the kernel in the *third* GiB of memory, and then the lowest free frame is one the kernel cannot write to. So the comment became structure:

```text
  go 1   free only  usable ∩ reach          reserve everything occupied
         ─ build all the page tables: every frame they take is in reach ─
  go 2   free       usable − reach          reserve everything occupied, again
         ─ switch ─
```

Two small functions in the library do the cutting, with tests:

```rust
pub fn inside(range: &Range<u64>, window: &Range<u64>) -> Option<Range<u64>>
pub fn outside(range: &Range<u64>, window: &Range<u64>) -> [Range<u64>; 2]   // below, above
```

Why the second "reserve everything": a reserved region may lie over memory the second go has just freed. And why it cannot hurt the tables: their frames are inside the reach, which the second go does not free; and marking a used frame used changes nothing.

On the PC this changes nothing visible (everything QEMU gives it is below 4 GiB or handled the same way), but the kernel no longer depends on a remark in a comment.

### A latent bug this exposed

The bitmap is placed by `find_free_space`: the first spot in usable memory that overlaps nothing reserved. "Reserved" was the kernel's own list - the image, the ramdisk, the boot info. It did **not** include regions the *map itself* calls unusable.

Firmware maps overlap. A PC's can say "usable: 1 MiB to 512 MiB" and also "reserved: 300 MiB to 301 MiB". The frame allocator handled that (not-usable wins). The bitmap search did not: had such a region been the first free spot, the bitmap would have been written over it.

It never happened on the PC because the first free spot is right after the kernel. On AArch64 a device tree's reserved list is exactly this shape - a reserved range *inside* the one memory range. The search now takes a function that lists everything occupied:

```rust
let occupied = || memory.taken.iter().cloned()
    .chain(core::iter::once(kernel.clone()))
    .chain(unusable());
```

(A *closure* - `|| ...` - that returns a fresh iterator each time it is called. The search calls it once per candidate spot. It is the no-heap way to say "all of these lists, joined": nothing is copied anywhere.)

## Changing the tables under running code

This is the part with no PC equivalent, and the part I learned most from.

On x86 the switch is `mov cr3, root`. The next instruction is fetched through the new tables. The new tables map the same code at the same address, so nothing notices.

AArch64 has a rule called **break before make**: an address must not go from one valid translation straight to a *different* valid one. The reason is the TLB, the CPU's memory of recent translations. The boot tables map the kernel with 2 MiB entries; the new ones with 4 KiB entries. If the CPU ever held both for the same address - an old 2 MiB one still remembered, a new 4 KiB one just looked up - the architecture says the result is not defined. A real CPU may raise a "TLB conflict" fault, or pick either.

So between old and new there must be a moment with **no** translation, during which everything remembered is forgotten. For most addresses that is easy. For the kernel's own code it is a puzzle: the code doing the change would have no translation either.

The way out was already there. The start code mapped the image's GiB *to itself* in the low half, through the other root register (`TTBR0`). The kernel's bytes are still reachable at their physical addresses. So:

```text
   high (TTBR1)                         low (TTBR0)
   ────────────                         ───────────
   mm::init
     install_kernel_tables(root)
        │  call, at its PHYSICAL address ──►  replace_kernel_tables:
        │                                        turn the kernel's half OFF   (TCR.EPD1)
        │                                        tlbi vmalle1   - forget everything
        │                                        TTBR1 = root   - the new tables
        │                                        turn the kernel's half ON
        │  ◄──────────────────────── ret ───  (to a high address: the new tables' first use)
     turn the LOW half off (TCR.EPD0), forget again
```

The routine is eleven instructions and touches no memory - there is no stack while the kernel's half is off. Every instruction is position-independent, so it runs the same at the address it was linked for and at the address it really is.

Calling it at its physical address, from Rust:

```rust
let low = kernel_physical(replace_kernel_tables as *const () as u64);
let replace: extern "C" fn(u64) = unsafe { core::mem::transmute(low as *const ()) };
replace(root);
```

(`transmute` reinterprets the bits of one type as another - here a number as "a function taking one `u64`". It is the most dangerous function in Rust and this is one of its few honest uses: I really do have the address of code and nothing in the type system can know that.)

After it returns, the low mapping has done its last job. It is turned off, and **address 0 faults**, as does every other low address, until there is a program to run there.

### The console across the switch

The serial port was mapped by `map_early_device` into a table that belongs to the boot tables. The new tables know nothing of it, and a kernel that goes silent at the moment it changes its page tables is not one I want to debug.

`map_early_devices` reads the old table's used entries and maps the same 2 MiB pieces, at the same addresses, in the new tables. The PL011 driver's pointer stays valid; it never learns anything happened. From then on `map_early_device` panics if called, and `mm::map_mmio` is the way.

### The address space afterwards

```text
 0x0000_0000_0000_0000   nothing. The low half is off.
 0xFFFF_8000_0000_0000   the direct map: all memory, read/write, never executable
 0xFFFF_C000_0000_0000   the heap
 0xFFFF_D800_0000_0000   kernel thread stacks (none yet on AArch64)
 0xFFFF_E000_0000_0000   devices mapped by mm::map_mmio (none yet)
 0xFFFF_FFFF_8008_0000   the kernel: code R+X, constants R, variables R+W
                         - with one page missing, under the boot stack
 0xFFFF_FFFF_C000_0000   devices mapped before all this: the serial port
```

The same numbers as on the PC, on purpose: both CPUs translate 48 bits, so the layout did not need to differ.

## The device tree, read twice

The tree was read through the low mapping. `Tree` and `Machine` hold *references* into it - Rust slices, which are an address and a length. After the switch those addresses translate to nothing.

The compiler cannot know that. A `&'static [u8]` at a low address is, to Rust, valid forever; I am the one who took the mapping away. So `kernel_main` reads the tree again, through the direct map, into variables with the same names:

```rust
let tree = arch::device_tree(device_tree).map(Tree::new);
let machine = match &tree { Some(Ok(tree)) => Machine::read(tree), _ => Machine::default() };
```

(*Shadowing*: a new `let` with an old name hides the old variable for the rest of the function. The dead references cannot be used by accident, because there is no longer a name to use them by.)

The PC kernel does the same with its `BootInfo` - it has since entry 4. `arch::device_tree` uses `phys_to_virt`, which is "physical + 0" before the switch and "physical + direct map" after, so one function serves both readings.

## The self-test, shared

`selftest.rs` was x86-only because most of it is about timers, threads and programs. Its first four checks are about memory and use nothing else. The file is now split: the memory checks stay in `selftest.rs` and run on both; the rest moved, unchanged, to `selftest/system.rs`, which is still PC-only.

Two checks are new, and run on both:

- **The top of memory.** The allocator hands out the lowest frame first, so no boot ever touched high memory - where a memory map read wrongly, or a direct map that stops short, would show. `allocate_highest` takes the highest free frame; the test writes 512 words to it and reads them back.
- **The kernel's own frames are not free** - first page, last page, a page of constants, and the page under the stack.

And on AArch64 the CPU is asked, as in entry 52 (`at`), about the *real* tables now rather than a test page: eight addresses of every kind must translate as the kernel's own code says; a write to code or constants must be refused; the low half must be gone.

## Design decisions

| Decision | Chosen | Rejected, and why |
|---|---|---|
| One `mm.rs` or two | One, asking `arch::mmu` seven questions | *An `mm_aarch64.rs`.* The seven differences are about fifty lines; the other five hundred would be a copy with none of the original's four dozen entries of use behind it. |
| What `init` is given | A `BootMemory` built by each `kernel_main` | *Convert the device tree into a fake `BootInfo`.* It would work, and would make AArch64 pretend to be booted by a loader it has never met - fields like "ACPI address" filled with zeroes. |
| Memory out of early reach | Hand it over in a second go | *Map all memory low in the start code.* One loop in assembly. But it would map every device in the first 512 GiB as ordinary cached memory, which on real hardware a CPU may read speculatively - with side effects. *Assert that table frames are in reach.* Detects the problem; does not solve it. |
| The table change | Through the low mapping, kernel half off in between | *Just write `TTBR1` and flush.* Works in QEMU, which does not model TLB conflicts. Breaks the architecture's rule on real CPUs. |
| How the kernel half is turned off | `TCR.EPD1`, one bit | *Point `TTBR1` at an empty table.* Needs a zeroed page at a known physical address; the bit needs nothing. |
| The early console | Re-mapped at the same address | *Map it afresh with `map_mmio` and tell the driver its new address.* A window in which printing is impossible, exactly where a failure most needs printing. |
| The low half afterwards | Off | *Leave it.* Then a null pointer with an offset reads kernel memory instead of faulting. |
| Frame bitmap | Still indexed from physical 0 | *Start it at the first byte of memory.* QEMU's memory starts at 1 GiB, so 32 KiB of bitmap describe nothing. On a board with memory at 512 GiB it would be 16 MiB. Worth doing; not in the same change as everything else. |
| Writable-implies-not-executable (`SCTLR.WXN`) | Not turned on | The tables already never map anything both ways. The bit would make the CPU enforce it; I want to see user programs load on this CPU first. |

## Bugs

**It booted the first time.** I note this because it has not happened before in this project, and because of what it does *not* mean. What I could verify by breaking it is in the table below. What I could not is under "Not tested", and one of those is the centre of the whole change.

**The bitmap could be placed on reserved memory** - described above. Found by reading, not by a failure; then confirmed by making the old behaviour come back (experiment 8) against a tree that reserves the exact spot.

**A check that could not fail.** I removed the line that keeps the device tree out of the allocator. Every boot passed. The tree sits 128 MiB into memory and nothing in a three-second boot allocates that far. My boot test's "about all the memory is free" allowed a megabyte or two of slack, and the tree is a megabyte.

So now the kernel asserts it: the tree's first and last bytes must be in frames that are not free. Same for the kernel's own image. `FrameAllocator::is_free` exists for this.

**And one I could only half fix.** Removing the line that keeps the *kernel image* out of the allocator: the kernel hangs silently rather than failing my new assertion. By the time the assertion runs, the heap's first megabyte has *already been given* the image's frames - so they are "not free", and the check passes while the kernel's code is being overwritten. The boot test catches it as "did not finish". A check placed before the heap would give a better message; the failure is at least not silent success.

## How I checked it

**Automated** (`make test64`):

- **Host tests**: 157 in the kernel's library, 8 new:
  - reserved memory gathered from the blob's list and from `/reserved-memory`; a child with no address, a disabled child and an ordinary device are not reserved; more ranges than fit are counted;
  - `inside` and `outside` at every position relative to a window; together they are always the whole range;
  - the free-space search with reservations from two chained lists; an empty range is in nobody's way;
  - `allocate_highest` across a word boundary of the bitmap, down to nothing; `is_free` inside a frame, past the end.
- **AArch64 boots**: ten ordinary ones. Each must now print every line at the top of this entry, with the numbers this machine should give - free memory within 3 MiB of all of it, and the highest free frame at exactly the last page:

  | Boot | What it adds |
  |---|---|
  | plain, el2, smp4, dirty, unnamed, clockless | as before, now through the whole memory start |
  | a53 | **6 GiB**: five of them given in the second go; the top frame is at `0x1bffff000` |
  | max | 128 MiB, another CPU |
  | **reserved** (new) | a tree that keeps the top 16 MiB *and the first 64 KiB* - where the bitmap would go. The bitmap must be at `0x40010000`, the top frame at `0x5efff000` |
  | **reserved-high** (new) | 6 GiB with the top 16 MiB kept: reserved memory in the second go |

- **AArch64 fault boots**: four. A null read (new); an unmapped read; a write to a constant and a jump into a variable - which now break the rules of the kernel's *real* tables, not of a test page.
- **x86_64**: the whole of `test64`. Every PC boot goes through the rewritten `init`, in two goes, and runs the two new self-tests.

**By hand - breaking it on purpose** (AArch64; each put back):

| # | Change | Result |
|---|---|---|
| 1 | the low half left on | panic: *the low mapping the kernel started on is still there* |
| 2 | no `tlbi` in the table change | **passes** - see below |
| 3 | constants mapped writable | panic: *rodata is P\|W\|G\|NX* |
| 4 | the page under the stack mapped | panic in the protections check |
| 5 | early devices not carried over | silence after the first `[memory]` line; the boot test times out |
| 6 | no second go (6 GiB) | boots with 1022 MiB; the boot test fails on the amount and the top frame |
| 7 | no second "reserve" (6 GiB, reserved top) | boots; top frame `0x1bffff000` - inside the reserved range; the boot test fails |
| 8 | bitmap search ignores the map's unusable regions | bitmap at `0x40000000`, on reserved memory; the boot test fails |
| 9 | `flush` does nothing | panic: *the address still reads the old one* |
| 10 | the kernel image not kept out of the allocator | hangs in the heap test |
| 11 | the device tree not kept out | passed, until the assertion was added; now a panic |

**Not tested, and why:**

- **Break before make** (experiment 2). The kernel without the `tlbi` passes every boot: QEMU never raises a TLB conflict, and - as far as I can tell from its behaviour - forgets its remembered translations by itself when the control register is written. So the sequence I am most careful about is the one I cannot show to be necessary. It is written from the rule in Arm's manual, and going through the low mapping for it is, as I understand it, what Linux does for the same job. That is an argument, not a test.
- **`commit_mappings`.** QEMU sees every table write at once. Removing the barriers changes nothing there.
- **An image that is not in the first GiB of memory**, or a tree in a different GiB from the image. QEMU's `-kernel` always puts them where it puts them. The two-go code is exercised (6 GiB), but not from that side.
- **A stack overflow.** The guard page is missing, as it should be, and the CPU agrees. But an overflow would fault *into the handler*, which pushes its frame onto the same stack and faults again, for ever. The PC has a separate stack for that case; AArch64 does not yet.
- **Kernel stacks and `map_mmio` on AArch64.** They compile and are the same code as on the PC. Nothing calls them yet.
- **`/reserved-memory`** on a booting kernel. Host-tested; the boots use the blob's list.

## What is still missing

- **Reporting a kernel stack overflow on AArch64** - a separate stack for exceptions. It belongs with threads (step 3e), where stacks get designed.
- **The frame bitmap wastes the hole below memory.** See the decisions table.
- **The boot tables are still in the image**: six pages of `.bss` nothing uses after the switch.
- **Translation through `ranges`** in the device tree is still not done.
- **Next**: step 3d - the interrupt controller (GIC) and the timer. For the first time the AArch64 kernel will map a device with `mm::map_mmio`.

## What I learned

- **A port finds the assumptions, not the bugs.** Seven things in `mm.rs` were true of a PC and written as if true of computers. Not one was wrong where it ran. I could not have listed them before trying to run the code somewhere else.
- **A comment that says "this works because..." is a debt.** "Frames come lowest first, from below 4 GiB" was true for 49 entries. It is now a loop that makes it true.
- **Not every CPU lets you swap the floor while standing on it.** x86 changes tables in one instruction. AArch64 needs a moment with no tables, and somewhere else to stand during it. The low mapping I had thought of as leftover scaffolding from the start code turned out to be that somewhere.
- **"It booted first time" measures the reading I did beforehand, not correctness.** Eleven experiments later I had found one check that could not fail and one bug that had been waiting since entry 4.
- **An emulator is forgiving exactly where hardware is not.** Zeroed memory (entry 50), no caches, and now a TLB that never conflicts and table writes that are seen at once. Every one of those is a place where "passes in QEMU" tells me nothing, and I should write the list down each time rather than discover it on a board.
- **Rust's `'static` means "as far as the compiler knows".** I gave it references into memory and then unmapped the memory. The type was still `&'static`. When *I* change what is mapped, the lifetimes are mine to keep - and shadowing the old names is a cheap way to keep them.

## Testing note

**1. The AArch64 kernel starts its memory manager**

```
make run-aarch64
```

Nothing to type. To leave QEMU: **Ctrl+A, then X**.

| Look for | Meaning |
|---|---|
| `[memory] 130737 frames free of 393216 (510 MiB), bitmap 48 KiB at 0x40000000` | all of the 512 MiB found, less the kernel, the tree and the tables |
| `[memory] own page tables at 0x4000c000: direct map 0xffff800000000000, NX on, write protection on` | the switch happened - and the console survived it |
| `[memory] heap at 0xffffc00000000000, 1024 KiB to start` | `Box`, `Vec` and `String` now work |
| `[tree] read again through the direct map: the same 56 nodes, in memory kept out of the allocator` | the device tree is still readable, where it now is |
| `[selftest] top of memory: the highest free frame, at 0x5ffff000, written and read back` | the last page of memory is really there |
| `[selftest] protections: code RX, rodata R, data RW+NX, null and stack guard unmapped` | each part of the kernel has only the permissions it needs |
| `[selftest] memory: all passed` | the same line the PC prints |
| `[paging] the low half is off: address 0 and 0x40080d9c, where this code really is, translate to nothing` | a null pointer now faults |
| `[kernel] this is as far as the AArch64 port goes - halting` | no panic on the way |

(Frame counts and the small addresses may differ by a little if the kernel's size changes.)

**2. More memory than the kernel can reach at first**

```
make run-aarch64 RUN_A64="-m 6G"
```

| Look for | Meaning |
|---|---|
| `[tree] memory      0x40000000 - 0x1c0000000 (6144 MiB)` | six GiB described |
| `[memory] 1572480 frames free of 1835008 (6142 MiB), bitmap 224 KiB at 0x40000000` | all six in the allocator - five arrived in the second go |
| `[selftest] top of memory: the highest free frame, at 0x1bffff000, written and read back` | a page above 4 GiB, reached through the direct map |

**3. Four rules broken on purpose**

```
make aarch64-test
qemu-system-aarch64 -M virt -cpu cortex-a72 -m 512M -nographic -kernel build/aarch64/Image-fault-null
qemu-system-aarch64 -M virt -cpu cortex-a72 -m 512M -nographic -kernel build/aarch64/Image-fault-rodata
qemu-system-aarch64 -M virt -cpu cortex-a72 -m 512M -nographic -kernel build/aarch64/Image-fault-nx
```

The first command should end `aarch64-test: all passed`, and leaves the kernels in `build/aarch64/`. Leave each QEMU with **Ctrl+A, then X**.

| Kernel | Look for |
|---|---|
| `Image-fault-null` | `[test] reading through a null pointer...` then `*** CPU EXCEPTION 0x25: data abort` and `kernel read at 0x0: not mapped (the level 0 table has no entry)` |
| `Image-fault-rodata` | `[test] writing to read-only data...` then `kernel write at 0xffffffff800911a8: protection violation (at level 3)` |
| `Image-fault-nx` | `[test] executing data...` then `*** CPU EXCEPTION 0x21: instruction abort` and `kernel instruction fetch at 0xffffffff80094000: protection violation (at level 3)` |

The two addresses are now inside the kernel itself (`0xffffffff800…`), not a test page; their last digits may differ.

**4. A machine that keeps memory back**

`make aarch64-test` leaves `build/aarch64/reserved.dtb`, a device tree that reserves the first 64 KiB and the top 16 MiB:

```
make run-aarch64 RUN_A64="-dtb build/aarch64/reserved.dtb"
```

| Look for | Meaning |
|---|---|
| `[tree] reserved    0x40000000 - 0x40010000` and `[tree] reserved    0x5f000000 - 0x60000000` | both reservations read |
| `bitmap 48 KiB at 0x40010000` | the bitmap moved off the reserved 64 KiB |
| `(493 MiB)` in the `frames free` line | sixteen fewer than before |
| `the highest free frame, at 0x5efff000` | just below the reserved top |

**5. The PC, on the same memory manager**

```
make run64
```

In the terminal it was started from, before the prompt:

| Look for | Meaning |
|---|---|
| `[selftest] frames: allocate, free, reuse; the kernel's own are not free` | the new check, on the PC |
| `[selftest] top of memory: the highest free frame, at 0x…, written and read back` | the new check, on the PC |
| `[selftest] memory: all passed` | as before |

| Type | You should see |
|---|---|
| `mem` | the memory summary, as before |
| `ps` | the list of processes |
| `cprimes 100` | `cprimes: 25 primes below 100` |

**6. Host checks**

```
cd kernel/kernel64 && cargo test --lib --target x86_64-unknown-linux-gnu
```

should end `test result: ok. 157 passed`.

Automated: `make test64`
