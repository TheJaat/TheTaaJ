# Era 16 · Memory: sharing, reclaim and hardening

*Part of the work list. The rules, who is doing what now, and what waits for the user are in [`docs/who-does-what.md`](../who-does-what.md); the lanes in [`docs/lanes.md`](../lanes.md).*


**Done when**: two programs started from one file share its code pages; a private file mapping is copy-on-write; a test that runs the machine out of memory finds a failed request or a stopped program, never a kernel panic, and the services still answering; SMEP and SMAP (x86) and PAN (AArch64) are on, with every boot test passing. Whose it is, and when, is for the user to say. J29 and J58 (program pages shared, sealed memory objects) stay in The-Jat's list, since Era 9's rustc needs them first; K25 (ASLR) stays in Security; E5 (swap) with the reclaim item.

What is right: one page-table implementation for both CPUs, a `Format` per CPU and the rest tested on the host (`paging.rs`); physical and virtual addresses as different types; a frame bitmap that starts all "in use", so a wrong memory map can only waste memory; kernel code read-only and data never executable (CR0.WP and NXE; UXN and PXN); memory objects shared without copying, where a mapping is a reference, so no page is freed while mapped; reserved memory filled on first touch; a guard page after every mapping; `mprotect` as a list of differences; every page a process takes counted against a limit that can only go down (`account.rs`); a documented lock order (HEAP, TABLES, FRAMES). Clean, portable and tested.

What it does not do yet:

- **Nothing is shared but explicit memory objects**: a program's file is loaded whole into each process (J29); no copy-on-write, so `MAP_PRIVATE` is a copy (entry 103).
- **Nothing is reclaimed**: no clean page dropped, no swap, no signal to services that memory is short.
- **The kernel heap panics when it can't grow** (`mm.rs`: a null from `alloc` becomes a panic) and never gives memory back. One request at the wrong moment can stop the machine.
- **Hardware protections off**: no SMEP or SMAP on x86, no PAN on AArch64, so a kernel bug can be made to run or read user memory; the kernel half is in every process's tables, with no KPTI for CPUs that leak it (Meltdown).
- **Not scaled**: the frame allocator scans a bitmap from the lowest frame under one lock; no 2 MiB pages for programs; a memory object is at most 512 MiB (`MEMORY_MAX`), less than Era 8's model weights; every first touch zeroes a new frame, even for a read.

All of it stays mechanism: the kernel shares, copies on write, and drops pages; **what to evict and when to ask a service to shrink** is policy, and goes to a user-space memory manager wherever it can.

| # | Item | Size | Notes |
|---|---|---|---|
| K84 | **SMEP and SMAP on x86, PAN on AArch64**: the kernel can't run user code and touches user memory only in its explicit copies (`user_pages`, `copy_from_user`, `copy_to_user`), with the bit lifted only there | S | First: small, and only security |
| K85 | **The kernel never panics for want of memory**: each kernel allocation that a request causes fails that request (`OutOfMemory`) instead; the heap gives back what it no longer uses; a test that runs the machine out of memory | M | Early: the era's milestone. Fallible allocation (`try_reserve`, `Box::try_new`) on every path a program can drive |
| K86 | **A memory-pressure signal**: an object services wait on, told when free memory falls under a level, so they drop caches before anything fails; which program to stop, if it comes to that, decided by a user-space manager, not the kernel | M | After K85 |
| K87 | **Copy-on-write memory objects**: a child object that shares its parent's pages until one is written - private file mappings, a program's data pages over its shared code | M | With J29 and J58 |
| K88 | **A generic pager**: a memory object whose missing pages are asked for through a channel, filled by a user-space service on a fault | L | Shared with Era 14 (J79); the kernel knows memory objects, never files |
| K89 | **Pages given back under pressure**: clean pages of paged objects dropped (their pager can fill them again), then E5's swap for the rest | L | After K88 and K86 |
| K90 | **A buddy allocator** for frames: allocation without a scan, contiguous blocks (DMA, huge pages) found at once | M | With K69's per-CPU caches |
| K91 | **2 MiB pages** for big mappings, kernel and programs | M | After K90. For Era 8's weights and big heaps |
| K92 | **A shared zero page**: reserved memory that is read before it is written maps one read-only page of zeros, and gets its own frame on the first write | S | After K87 |
| K93 | **Memory objects over 512 MiB** | S | For Era 8: Qwen3's 1-3 GB of weights |
| K94 | **Separate kernel tables** on CPUs that leak kernel memory to programs (Meltdown); off on those that don't | M | After K71 (PCID makes it cheap) |
