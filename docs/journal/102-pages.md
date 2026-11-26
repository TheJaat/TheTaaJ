# Memory for big programs, part 1: mprotect for real, memory given back, a heap that returns it, and copies that cannot race an unmap

*Journal entry #102 · The-Jat's step G, part 1 (`docs/who-does-what.md`), on the branch `big-memory`. Step E (LLVM 23) comes before this one in the order, but it is waiting on a decision only the user can make (see "Step E is waiting" below). Step G does not depend on it, so I did the half of G that is the kernel's and libc's own:
- **J26, `mprotect` and `PROT_NONE`** really change what pages allow;
- **J27, `MADV_DONTNEED`**: give pages back from inside a reservation;
- **J30, the unmap race**: a system call copying into a page another thread is unmapping;
- **J31, small malloc classes give memory back**, and so does the heap under them.

J28 (shared file mappings that write back) and J29 (programs loaded page by page) are part 2.*

## Why

Big programs manage memory themselves and expect the kernel to follow:
- **`mprotect` changed nothing.** It refused `PROT_EXEC` and otherwise returned 0. A program that put a guard page below a buffer, or reserved address space with `PROT_NONE` to commit later (Rust's std, LLVM's JIT-less allocators, every arena allocator), got no fault where it counted on one. Worse, one that relied on `PROT_READ` to catch stray writes never saw them.
- **Memory never went back while a program ran.** `madvise(MADV_DONTNEED)` was ignored. `munmap` of part of a mapping left it whole. The heap kept every page it ever touched. rustc's peak is 426 MB (entry 096), but most of that is short-lived: a compiler that frees between phases should drop back, not sit at its peak until it exits.
- **libc's small blocks never left their lists.** Two hundred thousand freed 48-byte blocks stayed 12 MiB of memory for good.
- **The race noted in entry 096.** A system call found the physical pages of a user buffer under the process's locks, let the locks go, then copied. In between, another thread could unmap the buffer, its frames be handed to someone else, and the copy land in memory no longer the program's.

## How it fits together

```
 program (libc)                               kernel
 ─────────────────────────────────────        ──────────────────────────────────────────────────
 mprotect(p, n, PROT_x)  ──────────────────►  MEMORY_PROTECT(p, n, none|read|read-write)  (43)
                                                a reservation's Protections (protections.rs):
                                                  one usual access + sorted ranges that differ
                                                pages already touched: flags changed in place
                                                  none  -> mapped for the kernel only (no USER):
                                                           bytes kept, any user touch faults,
                                                           every system call refuses (BadAddress)
                                                  read  -> USER, not WRITABLE
                                                then every CPU forgets the old flags (shootdown)
 untouched page touched  ──── page fault ───►  fill: Protections.at(address) allows it? map a
                                                zeroed frame with those flags : SIGSEGV
 madvise(p, n, MADV_DONTNEED) ─────────────►  MEMORY_DISCARD(p, n)  (44)
 munmap(part of a mapping) = discard + none     pages unmapped, CPUs told, frames freed,
                                                uncharged; the reservation stays
 free(>= 64 KiB) in the heap ──────────────►  MEMORY_DISCARD of the whole pages it freed
 free(small) -> its piece empties -> the piece   (heap::pages_to_give_back)
   goes back to the heap as a 64 KiB free

 read(), getrandom(), ... ─────────────────►  with_user_pages(p, n, write, |pieces| copy):
                                                the copy runs with the mappings lock held, so
                                                no unmap, discard or protect can come between
```

**Protections.** A reservation (MEMORY_RESERVE) used to remember one thing, whether it was writable. Now it keeps a `Protections`: the access it was made with, and a sorted list of ranges that differ. `set` cuts the ranges a new one overlaps, drops what matches the usual access, and joins neighbours with the same access. So putting a range back as it was leaves nothing behind, and the list stays as short as the program's real pattern of protections. A fault on an untouched page asks `Protections::at`. A page with no access is never filled, so touching it is SIGSEGV, as on Unix.

**Pages already touched** get their page-table flags changed in place: unmapped and mapped again to the same frame with new flags. That needs no new page-table operation, and no frame, because the tables for the page are there already. For `PROT_NONE` the trick is to keep the page mapped *without USER*:
- the frame stays the program's, so its bytes are still there when access comes back;
- the CPU refuses any user-mode access, a protection fault, which goes straight to SIGSEGV (only not-present faults go through `fault_in`);
- every system call refuses it too, because `with_user_pages` has always demanded USER.

On AArch64 such a page is EL1-only. It is also never executable at EL1, because NO_EXECUTE sets both PXN and UXN.

**Discarding** takes a range of a reservation and does what `unmap_memory` does for a whole one: the pages leave the tables, every CPU forgets them, and only then are the frames freed and uncharged. The address space stays reserved, so the next touch is a fresh zeroed page.

**libc.** `mprotect` and `madvise(MADV_DONTNEED)` pass straight to the kernel, which checks the range: whole pages, all in one reservation. That covers mmap's mappings, malloc's memory and threads' stacks, but not the main thread's stack (ENOMEM, Linux's "not mapped"). `mmap` with `PROT_NONE` or `PROT_READ` reserves, fills (for a file), then protects. `munmap` of part of a mapping now gives that part's memory back and puts it out of reach, as an unmapped page would be. The address space stays until the whole mapping goes.

**The heap** (`user/src/allocator.rs`, the Rust allocator under libc's malloc). `Heap::deallocate` now says which free block the memory joined. When 64 KiB or more is freed, the whole pages that free covers inside that block are discarded (`heap::pages_to_give_back`). The block's first 16 bytes stay, because the free list's header lives there. This happens before the heap's lock is let go: after that, another thread may be handed those pages, and discarding them would wipe what it wrote.

**Small blocks** (libc's malloc, up to 1 KiB) were cut from shared 64 KiB pieces and kept on one free list per size, for ever. Now each piece serves one size, sits on a multiple of 64 KiB, and starts with a header: its size, how many of its blocks are in use, its own free list. So the piece a block belongs to is its address with the low 16 bits cleared. Freeing the last block in use sends the piece back to the heap, unless it is the only piece its size has. That is a 64 KiB free, so its pages go back to the kernel.

**The race.** `user_pages` became `with_user_pages`, which runs the copy as a closure while the process's mappings and tables are locked. Unmapping, discarding and protecting all take the mappings lock first, so none can come between finding a frame and copying to it. The futex word accessors do their atomic operation the same way. The copies are short (a message, at most RANDOM_MAX, through the direct map that never faults), so holding the lock for them costs little.

## Design decisions

- **`PROT_NONE` as "mapped for the kernel only", not unmapped.** Unmapping would lose the bytes, and `mprotect(PROT_NONE)` followed by `mprotect(PROT_READ|PROT_WRITE)` must give them back. Keeping the PTE without USER keeps them, costs nothing extra, and reuses the check every system call already makes.
- **The protections live in the reservation, not only in the page tables.** Untouched pages have no PTE to carry them, and filling every page of a big `PROT_NONE` range just to hold its flags would defeat the point of reserving.
- **Ranges, not a flag per page.** Programs protect big ranges (a 64 MiB arena) and single pages (a guard). A list of ranges is small for both; a bitmap per page would cost per page of every reservation.
- **The heap gives back only on frees of 64 KiB or more, and only what was just freed.** A system call on every small free would cost more than the memory. Looking only at what was just freed bounds the work by the free's own size: the rest of a big free block went back when *it* was freed, and is not walked again.
- **One spare piece per size.** A program that takes and frees one small block in a loop would otherwise make and break a piece each time.
- **`munmap` of part keeps the address space.** Splitting a reservation would mean a region list that splits and merges. Discarding plus `PROT_NONE` gives the program everything it can see (memory back, touches fault), and the address space is cheap: 128 TiB of it.
- **`MADV_DONTNEED` on a mapped file reads back zeros**, not the file's bytes as on Linux. A mapping here is a private copy read in once, and the kernel does not know the file. sys/mman.h says so.

## Step E is waiting

LLVM 23's libc++ needs a clang of 21 or later to compile, and the host has Ubuntu's clang 18. The byte-for-byte checks (clang on TheTaaJ against the host's) need the host's clang to be the same release as the port. So step E starts with building LLVM 23.1.5 for the host, and that needs cmake and ninja, which this machine does not have. There is no sudo without a password and no pip. I downloaded Kitware's cmake 3.31.6 release (its SHA-256 checked) and ninja 1.12.1 into `build/host-tools/dl`. Unpacking and running downloaded binaries was refused by the session's permission check, and I did not work around that. The user can:
- install `cmake` and `ninja-build` with apt;
- or allow running those two downloaded tools;
- or install LLVM 23 from apt.llvm.org, and the step then uses it.

## Bugs

- **The first boot's "killed" check failed the test** although all 54 checks passed. The boot's own self-test kills its `forever` programs ("killed by request"), and the check matched any "killed". It now matches only `"pages" killed`.
- **A check that checked nothing**, caught while writing the test: `getrandom` into a buffer, then a zero-length `memcmp`, labelled "a system call may read from a read-only page". It is now a pipe write from the read-only page, read back and compared.

## How I checked it

- Host tests: `protections.rs` (7 tests: a guard page and a read-only stretch, splitting and joining, back to the usual access, a range over several), `regions.rs` (`find_mut`), the heap (`deallocate` saying which block the memory joined, `pages_to_give_back` with aligned, unaligned, mid-page and too-small cases). The kernel's 225 host tests and the heap's 9 pass.
- `pages.c` with `pages-test.sh`, on x86_64 and AArch64 (`pages64-test`, `aarch64-pages-test`, in `test64`), 54 checks:
  - **mprotect**: a page out of reach faults on read and write, its neighbours don't, a system call refuses it with EFAULT, and access back keeps its bytes; read-only pages read, refuse writes (by the program and by a system call), and can still be read by a system call (a pipe write); PROT_EXEC is EACCES, an unaligned address EINVAL, the main stack and past a mapping's end ENOMEM; a PROT_NONE mapping brought in a part at a time; a PROT_READ mapping.
  - **madvise and munmap**: 64 MiB touched counts, 16 MiB given back counts 16 MiB less and reads zeros, the rest is untouched; 8 MiB unmapped from the middle counts less and faults; the whole mapping unmapped leaves less than 1 MiB counted.
  - **the heap**: 32 MiB malloc'd, touched and freed goes back; 200 000 blocks of 48 bytes (12.5 MiB) freed give back at least 11 MiB, and can be had again.
  - **the race**: a thread discards, protects and unprotects a page 20 000 times while the main thread `getrandom`s into it. Every copy either went in or was refused whole: 9 168 in and 10 832 refused on x86_64, 11 850 and 8 150 on AArch64.
- Mutations, each put back afterwards:
  - Pages already touched left with their old flags: 8 checks fail (no faults, a system call writes the "read-only" page, bytes changed).
  - `fill` ignoring the protections: the untouched PROT_NONE and PROT_READ pages fill and are writable (3 checks).
  - Discard doing nothing: "16 MiB less counted", "zeros there next time", "8 MiB less counted" fail.
  - The heap never giving back: "freed, it goes back", "freed, their pieces go back" fail.
  - The race fix can't be shown failing by a test. The race check proves only that copies are whole and nothing breaks. Copying after the locks are let go would pass it too, unless a freed frame happened to be handed to someone else in that window. The fix is argued from the lock order: every path that frees a frame takes the mappings lock first.
- `make test64`: all passed in 31 min 42 s, on the first run (55 parts; slower than entry 101's 15 minutes, by the two new boots and a busier machine). The boot64 "did not run all 237 commands" warning is the soft one from entry 100.

## What is still missing

- **J28**: `MAP_SHARED` file mappings that write back.
- **J29**: programs loaded page by page from their file.
- **Small frees don't give back pages** unless their piece empties, and medium frees (under 64 KiB) never do.
- **`munmap` of part of a mapping keeps its address space** until the whole mapping is unmapped.
- **No `PROT_EXEC`**: a program still can't make memory it can run (no JITs).

## What I learned

- **"Present but not user" is a third state between mapped and unmapped**, and it is exactly what `PROT_NONE` on a touched page needs: the hardware enforces it, the bytes survive, and the kernel's own buffer checks already honour it.
- **The order of a free matters as much as what it frees.** Discarding the heap's pages after letting go of its lock would be a bug that shows only under threads: another thread's fresh allocation wiped to zeros.
- **A slab-style piece header** turns "which piece is this block in" into one AND, once pieces are aligned to their own size. That makes counting blocks per piece, and so returning empty pieces, cheap.
- **Rust:** a closure parameter (`impl FnOnce(&[(u64, usize)]) -> R`) to keep a lock held for exactly the work that needs it. The lock's guard lives in `with_user_pages`'s frame, the closure can't take it out, and the borrow checker stops the closure from smuggling the page list out past the guard. That is the bug J30 was, now unrepresentable.
- **Rust:** `Regions::find_mut` returns `(&Range, &mut T)` from one `&mut` borrow of the vector element. Splitting a `&mut (A, B)` into `&A` and `&mut B` is allowed because they are different fields.

## Testing note

**1. The PC**

```
make run-disk
make run64
```

Close QEMU before `make run-disk`. It puts `pages` on the disk.

| Type | You should see |
|---|---|
| `with programs pages` | after some seconds `pages: N copies went in, M were refused` (the two add up to 20000) and `pages: 54 checks, all passed`, then `"pages" exited with code 0` |
| `mem` before and after | the memory in use back where it was: what pages took, it gave back |

**2. AArch64**

```
make run-aarch64-disk
make run-aarch64
```

The same as part 1.

**3. The tests by themselves**

```
make pages64-test
make aarch64-pages-test
make test64
```
