# Page Tables in Two Dialects

*Journal entry #52 · Era 3, step 3c, first half. The page-table code that has run the x86_64 kernel since entry 4 learns to write AArch64 entries too - one implementation, two formats - and on AArch64 it is checked against the only judge that matters: the CPU is asked what it makes of the tables, and must agree.*

```
[paging] the tables the CPU is using, read by the kernel: 5 lookups, all as the CPU does them
[paging] a page at 0x400b4000 mapped twice: written at 0xffffffffffe00000, read at 0xffffffffffe01000, where the CPU refuses a write
[paging] a third address unmapped, then mapped to the page at 0x400b6000: the CPU reads the new page there
```

---

## Why

Step 3c is memory on AArch64: a frame allocator, the kernel's own tables with real permissions, a heap. All of that sits on one module, `kernel64::paging`: `map`, `unmap`, `translate`, making and destroying an address space. It is 400 lines with 12 host tests, and every process the x86 kernel has ever run was mapped by it.

Its first line used to be:

```rust
//! x86_64 page tables: the map from virtual to physical addresses.
```

and in `lib.rs` it was the only module marked `#[cfg(target_arch = "x86_64")]`.

So before any memory management on AArch64 there is a decision to make about this module: write a second one, or teach this one a second language. This entry is that decision and nothing else. The frame allocator and the heap are the next entry.

## How alike the two are

Both CPUs translate a 48-bit address with a tree four levels deep. Each table is one 4 KiB page of 512 eight-byte entries. Nine bits of the address choose the entry at each level; the last twelve are the offset in the page. An entry above the last level can be a big page (2 MiB, 1 GiB) instead of a pointer to the next table.

Every word of that is true of both. The *walk* is the same walk. What differs is what the bits of one entry mean:

| Meaning | x86_64 | AArch64 |
|---|---|---|
| in use | bit 0 | bit 0 |
| this is a big page, not a table | bit 7 **set** | bit 1 **clear** |
| may be written | bit 1 **set** | bit 7 **clear** ("read-only" is the bit) |
| programs may use it | bit 2, needed at **every level** | bit 6, in the last entry only |
| shared by all address spaces | bit 8 set ("global") | bit 11 **clear** ("not global" is the bit) |
| may not be executed | bit 63 | **two** bits: 53 for the kernel, 54 for programs |
| a device, not memory | two cache bits (3 and 4) | a number (bits 2-4) choosing a kind from a register |
| has been used | set by the CPU | **must be set by the kernel**, or the first use faults |
| the address | bits 12-51 | bits 12-47 |

Three of these are the same idea with the sense turned round. One (execute) is one bit against two. One (the accessed bit) is a trap: forget it and every page faults on first touch.

## One implementation, a `Format`

The callers - `mm.rs`, `process.rs`, the memory objects - never wrote entry bits. They say what they want:

```rust
tables.map(virt, phys, PageSize::Size4K, Flags::WRITABLE | Flags::USER | Flags::NO_EXECUTE, frames)
```

`Flags` is the kernel's own vocabulary. Its numbers happen to be x86's bits, because that is where it began, but nothing outside `paging.rs` depends on that. So `Flags` stays, as the *request*, and a new type says how a request becomes an entry:

```rust
pub enum Format {
    X86_64,
    AArch64,
}

impl Format {
    pub const NATIVE: Format = if cfg!(target_arch = "aarch64") { Format::AArch64 } else { Format::X86_64 };

    fn is_present(self, entry: u64) -> bool
    fn is_table(self, entry: u64) -> bool
    fn address(self, entry: u64) -> u64
    fn table_entry(self, frame: PhysAddr, user: bool) -> u64
    fn table_entry_for_user(self, entry: u64) -> u64
    fn leaf_entry(self, phys: PhysAddr, size: PageSize, flags: Flags) -> u64
    fn flags(self, entry: u64, big: bool) -> Flags
}
```

Seven small functions hold *everything* that differs. `map`, `unmap`, `translate` and the rest call them and contain no bit of either CPU. A `PageTables` remembers its format.

The AArch64 half of `leaf_entry` is where the table above turns into code:

```rust
let mut entry = phys.as_u64() | a64::VALID | a64::ACCESSED;
if size == PageSize::Size4K {
    entry |= a64::TABLE_OR_PAGE;          // clear = a block
}
if user {
    entry |= a64::USER;
}
if !flags.contains(Flags::WRITABLE) {
    entry |= a64::READ_ONLY;              // the sense is turned round
}
if !flags.contains(Flags::GLOBAL) {
    entry |= a64::NOT_GLOBAL;             // and here
}
...
// Execution is allowed to at most one side.
entry |= if flags.contains(Flags::NO_EXECUTE) {
    a64::NO_KERNEL_EXECUTE | a64::NO_USER_EXECUTE
} else if user {
    a64::NO_KERNEL_EXECUTE
} else {
    a64::NO_USER_EXECUTE
};
```

That last part is a small gain from having two bits. On x86 a page that is executable is executable by whoever can reach it. Here a page the kernel may run is *never* runnable by a program, and the kernel can *never* run a program's page - not as a policy somebody has to remember, but because of how one function writes the bits.

### Why an `enum` and not `cfg`

The obvious way is `#[cfg(target_arch = ...)]`: compile one set of functions or the other. Then the AArch64 half would never be compiled on my laptop, and my laptop is where the tests run. With a value, the host tests ask for the other format by name:

```rust
let tables = unsafe { PageTables::new_in(&mut memory, base, Format::AArch64) }.unwrap();
```

and the AArch64 entries are checked, bit for bit, on a PC. The cost is one `match` per entry written. `NATIVE` is a constant, so where the format is known the compiler removes even that.

### Two things that are not just bits

**A process's address space.** On x86 there is one root, so every process's root has to contain the kernel: `new_user` copies the kernel's upper 256 entries. On AArch64 there are two root registers. The kernel's tree stays in `TTBR1_EL1` for ever; a process is a separate tree in `TTBR0_EL1`. So for `Format::AArch64`, `new_user` copies nothing, and `fill_kernel_slots` (which exists only to make that copy safe) does nothing. This is not used yet - there are no processes on AArch64 - but it is tested.

**Adopting a tree.** `PageTables::new` makes an empty tree. The AArch64 kernel already *has* one: `entry.rs` built it in assembly, and the CPU is running on it. `PageTables::adopt(root, offset, format)` wraps a tree that exists, so the ordinary code can add to it. That is how this entry tests on a real CPU with no frame allocator: it adopts the live boot tables and maps into them, taking the few pages it needs from a pool of eight in the kernel's `.bss`.

## Asking the CPU

Host tests can show that the code writes bit 7 for "read-only". They cannot show that bit 7 *means* read-only. For that there is exactly one authority.

AArch64 has an instruction for it: **`at`**, "address translate". It does the walk the CPU would do for a real access, permission checks included, and instead of faulting puts the result in a register (`PAR_EL1`). There are variants: `at s1e1r` asks "could the kernel read this?", `at s1e1w` "could it write?".

```rust
pub fn cpu_translate(virt: u64, write: bool) -> Option<u64>
```

The self-test, at every boot:

1. **Reading.** Five addresses - a function, a variable, a device register, and two that are not mapped. For each, `tables.translate(address)` and `cpu_translate(address)` must give the same answer. The tables here were written by *assembly*, not by this code, so this checks that the code reads the format the CPU reads.
2. **Writing.** One page mapped at two addresses: writable at one, read-only at the other. A number written through the first must be read through the second. Then the CPU is asked: write through the first? Yes. Read through the second? Yes. Write through the second? **No.**
3. **Changing.** A third address is mapped to that page and read. It is unmapped, the CPU is told to forget it (`tlbi`), and it is mapped to a *different* page. A read must now see the different page.

Two rules are left that `at` cannot probe, and those get a fault boot each, as on x86: a kernel that writes to the read-only address, and one that jumps to the writable (so not executable) one.

```
*** CPU EXCEPTION 0x25: data abort
    kernel write at 0xffffffffffe01000: protection violation (at level 3)

*** CPU EXCEPTION 0x21: instruction abort
    kernel instruction fetch at 0xffffffffffe00000: protection violation (at level 3)
```

## Design decisions

| Decision | Chosen | Rejected, and why |
|---|---|---|
| One module or two | One, with a `Format` | *A second `paging_aarch64.rs`.* 400 lines copied, of which seven functions differ. Every later fix would have to be made twice, and the second copy would have none of the first's history of being right. |
| How the format is chosen | A value in each `PageTables` | *`cfg`.* The AArch64 half would not exist on the machine the tests run on. *A generic parameter `PageTables<F>`.* The same testability, but every type that holds a `PageTables` would grow a parameter. |
| `Flags` | Kept as it is, x86's numbers and all | *New neutral numbers.* Cleaner on paper; it would change every entry the x86 kernel writes for no gain, and "bit for bit as before" is the property I most want from this change. |
| A process's root on AArch64 | Its own tree; nothing copied | *Copy the kernel half anyway.* It would put kernel tables where a program's high addresses are translated. Harmless only because of permission bits - not a thing to rely on. |
| Checking against the CPU | The `at` instruction, plus two fault boots | *Only fault boots.* Each is a build and a boot for one fact. `at` gives six facts in one ordinary boot. |
| Where early table pages come from | Eight pages in `.bss` | *Wait for the frame allocator.* Then this code's first run on a real CPU would be the same change as the allocator's, and a failure could be either. |
| The accessed bit | Always set | *Leave it clear and handle the fault.* That is how a kernel learns which pages are used - for swapping, which does not exist. |

## Bugs

**The code was right the first time; my testing of it was not.** After the change, I broke things on purpose as usual. Breaking the flush (`tlbi`) gave a surprise: the kernel died *earlier*, in step 2, with "not mapped". A missing flush cannot cause that.

It had not. My script for these experiments changes one file, builds in a scratch directory, and puts the file back. It put it back with `mv`, which keeps the file's old modification time - older than the build just made. The experiment before had been in `paging.rs`. So when the next experiment touched a different file, `cargo` saw nothing new in `paging.rs` and reused the *broken* library from the experiment before. I was testing two breakages at once and reading the result as one.

The script now touches the files it restores. I went back over the experiments of entries 50 and 51: there each change was in the same crate as the one before, which is rebuilt whole, so those results stand. This one would not have.

**And then the real answer: the flush was not tested.** Run properly, the kernel without `tlbi` passed everything. Step 3 at that time unmapped a page and asked the CPU, with `at`, whether it still translated. But `at` *walks the tables*; it does not consult what the CPU remembers. So it said "no" whether or not anything was forgotten.

What a stale translation actually does is keep reading the *old page*. So step 3 now maps the address to a different page and reads through it. Without the flush:

```
*** KERNEL PANIC: after being mapped to another page, the address still reads the old one
```

(QEMU does model this. I had assumed it might not.)

## How I checked it

**Automated** (`make test64`):

- **Host tests**: 149 in the kernel's library; `paging` has 20, 8 of them new:
  - x86 entries bit for bit what they were: `0x8000000000007103` for a kernel data page, `0x8000000040000183` for a 2 MiB one;
  - AArch64 entries against values worked out by hand from Arm's manual: kernel code, read-only data, data, not-global, a device, a framebuffer; a program's code and data; a 2 MiB block;
  - nine combinations of flags written and read back the same;
  - map, unmap, already-mapped, out of frames, a block in the way - in the AArch64 format;
  - a process's tree made and destroyed with every frame returned, and the kernel invisible in it;
  - an adopted tree is the same tree.
  - The 12 older tests run unchanged.
- **AArch64 boots**: all eight ordinary boots must print the three `[paging]` lines. Three fault boots now: unmapped read, write to read-only, execute data.
- **x86_64**: the whole of `test64` as before. Every page table of every process in those boots is written by the changed code.

**By hand - breaking it on purpose** (AArch64 format; each put back):

| Change | Host tests | Boot |
|---|---|---|
| "read-only" is never written | 3 fail | panic: *the CPU would allow a write to the read-only page* |
| the accessed bit is not set | 3 fail | data abort at the first use of the page |
| a 4 KiB entry written as a block | 2 fail | data abort: *not mapped* (a block is not valid at the last level) |
| `tlbi` removed | - | **passed**, until step 3 was rewritten; now panics |

**Not tested:**

- **The kind of memory** (ordinary / device / uncached). The bits are checked against the manual on the host. QEMU treats all three alike, so no boot can tell.
- **User entries on a CPU.** Their bits are host-tested; there is no program to run yet.
- **Two CPUs.** `flush` uses the form of `tlbi` that reaches every CPU; only one is running.
- **1 GiB blocks**: `translate` reads them; `map` cannot make them, on either architecture.

## What is still missing

- **Everything else of step 3c**: the frame allocator on AArch64, the kernel's own tables (text read-execute, read-only data, data), a direct map, the heap. `mm.rs` is still marked x86_64-only. The eight-page pool and the adopted boot tables are scaffolding for this entry and the next.
- **`Flags` still has x86's numbers.** Harmless, and a little odd to read.
- **Address-space identifiers.** AArch64 can tag each remembered translation with a process number so that switching processes needs no flush. "Not global" is written correctly for it; nothing uses it.

## What I learned

- **The two architectures disagree about spelling, not about grammar.** I expected page tables to be the hard part of the port. The walk needed no change at all; a table of nine rows covers the differences.
- **Three "opposite sense" bits are three chances to be exactly wrong.** Writable/read-only, global/not-global, huge/table. Each is the kind of mistake that still boots. That is why the expected values in the tests are worked out from the manual and written as numbers, not computed by a second copy of the same logic.
- **A value instead of `cfg` buys testability.** The rule I take from it: use `cfg` for what *cannot* compile elsewhere (an instruction), and a value for what merely *differs*.
- **The CPU can be asked.** `at` turns "I believe this entry means read-only" into an assertion that runs at every boot. x86 has nothing like it.
- **Know what the probe looks at.** `at` reads the tables; the bug it could not see lives in what the CPU remembers. A check must fail *through the same path* as the fault it guards against.
- **My tools need the same suspicion as my code.** An experiment whose setup I did not check gave me a confident, wrong observation. The clue was that the result made no sense - and the right reaction to that was to distrust the experiment before theorising about the kernel.

## Testing note

**1. The AArch64 kernel checks its page tables against the CPU**

```
make run-aarch64
```

Nothing to type. To leave QEMU: **Ctrl+A, then X**.

| Look for | Meaning |
|---|---|
| `[paging] the tables the CPU is using, read by the kernel: 5 lookups, all as the CPU does them` | the shared code reads AArch64 tables as the CPU does |
| `[paging] a page at 0x400b4000 mapped twice: written at 0xffffffffffe00000, read at 0xffffffffffe01000, where the CPU refuses a write` | what it writes means what was asked: one page, two addresses, two permissions |
| `[paging] a third address unmapped, then mapped to the page at 0x400b6000: the CPU reads the new page there` | unmapping, and making the CPU forget, work |
| `[kernel] this is as far as the AArch64 port goes - halting` | no panic on the way |

(The two page addresses may differ if the kernel's size changes.)

**2. Two rules broken on purpose**

```
make aarch64-test
qemu-system-aarch64 -M virt -cpu cortex-a72 -m 512M -nographic -kernel build/aarch64/Image-fault-rodata
qemu-system-aarch64 -M virt -cpu cortex-a72 -m 512M -nographic -kernel build/aarch64/Image-fault-nx
```

The first command should end `aarch64-test: all passed`, and leaves the two kernels in `build/aarch64/`.

| Kernel | Look for |
|---|---|
| `Image-fault-rodata` | `[test] writing to read-only data...` then `*** CPU EXCEPTION 0x25: data abort` and `kernel write at 0xffffffffffe01000: protection violation (at level 3)` |
| `Image-fault-nx` | `[test] executing data...` then `*** CPU EXCEPTION 0x21: instruction abort` and `kernel instruction fetch at 0xffffffffffe00000: protection violation (at level 3)` |

Each ends with a register dump and nothing after it.

**3. The PC, whose page tables the same code now writes**

```
make run64
```

| Type | You should see |
|---|---|
| `mem` | the memory summary, as before |
| `ps` | the list of processes - each one an address space made by the changed code |
| `cprimes 100` | `cprimes: 25 primes below 100` |

**4. Host checks**

```
cd kernel/kernel64 && cargo test --lib --target x86_64-unknown-linux-gnu paging
```

should end `test result: ok. 20 passed`.

Automated: `make test64`
