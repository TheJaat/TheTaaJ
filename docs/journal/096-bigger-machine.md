# A bigger machine: memory taken as it is touched, a limit for each program, and rustc measured

*Journal entry #96 · Era 9, steps R0 and R1. Before rustc can run on TheTaaJ, two questions: what does it need, and can TheTaaJ give it?
- **R0, on the host:** rustc 1.100.0 built from source as **one 182 MB program**, LLVM inside, no shared libraries. It compiles our crates in at most **426 MB**, and makes the same bytes as rustup's rustc. The toolchain is pinned to 1.100.0.
- **R1, in the kernel:** memory can be **reserved**, and a page gets memory only when it is first touched. Stacks are 8 MiB by default and can be a gigabyte. `mmap` works for real, every program has a **memory limit**, and programs may be 512 MiB.
- **x86 TLB shootdown:** since entry 095 a program's threads run on several CPUs at once. On x86 a page unmapped on one CPU could stay usable on another, so x86 now interrupts the others to forget it.

At 4 GB, a program touches 3 GiB in 1.9 s and gives it all back.*

## Why

The user asked for R0 and R1 (`docs/era-9-rustc.md`). R0 decides what the later steps must build: a dynamic loader or not, how much memory, which parts of the C library. R1 gives programs the memory rustc and LLVM expect from Linux:
- big stacks;
- `mmap` of big areas they only partly use;
- a limit, so that one big compile cannot starve the services.

## R0: what rustc needs (on the host)

I built rustc 1.100.0 from its source tarball with Rust's own bootstrap (`x.py build --stage 1 compiler/rustc`). The setup:
- the same LLVM rustc bundles (23.1.5), targets X86 and AArch64;
- one change: `compiler/rustc_driver/Cargo.toml` says `crate-type = ["rlib"]` instead of `["dylib"]`, so the compiler is linked into the `rustc` program instead of living in `librustc_driver.so`;
- the cmake and ninja the build wants, from their release archives (checked by SHA-256).

It took 18 minutes on 12 jobs, and the build itself peaked at 2.3 GB. All of it is under `build/r0`, outside git.

What came out:

| Question | Answer |
|---|---|
| Can rustc be one program? | **Yes.** 182 MB, 144 MB stripped, of which 141 MB is code. It needs libc, libm, libz and libgcc_s, nothing else. So R4 needs **no dynamic loader** |
| Is it the same compiler as rustup's? | Only after `build.description = ""`. A build from the tarball adds "(built from a source tarball)" to its version, and then refuses every crate rustup's 1.100.0 built (E0514). With it, the version strings are equal and the crates are accepted |
| Peak memory, one thread, `codegen-units=1` | The kernel binary **426 MB, 5.0 s**; libc's crate 272 MB, 4.2 s; every program and library under 190 MB |
| Same bytes? | Built by both compilers with the same flags: the 53 programs, `libc.a` and the kernel are **identical**. One difference: inside `libtaaj.rlib`, the LLVM bitcode names its anonymous globals `anon.<hash>.N`, and the hash differs, though the modules' contents and recorded hashes are equal. Nothing linked is affected |
| LLVM | rustc 1.100.0 needs LLVM 21 or newer. TheTaaJ's LLVM port is 18.1.3, so R4 builds LLVM 23 for TheTaaJ as part of rustc |
| The C library | rustc and its LLVM import 422 functions from the system's C library; ours lacks 190. Most are glibc's private names (`*64`, `__xstat`, `_l` locale variants) that a `taaj` target won't ask for. The real list for R3: `dlopen`/`dlsym`/`dladdr`, `dl_iterate_phdr`, `getaddrinfo`, `getrandom`, `sigaltstack`, `poll`, `pthread_getattr_np`, `pthread_attr_getstack`, `posix_spawn_file_actions_addchdir`, `sched_getaffinity`, `statx`, `copy_file_range`, wide characters, iconv, and zlib's `compress2`, `crc32` and `uncompress` |
| Memory in the kernel | Already fine: with 4 GB, x86 has 4092 MiB free after boot and AArch64 4093. The highest frame was written and read back. What was missing was memory for *programs* |

To measure each crate I used a `RUSTC_WRAPPER` (`build/r0/measure-rustc.sh`). It runs the stage-1 rustc against rustup's sysroot, adds `-C codegen-units=1 -Z threads=1`, and records each crate's peak memory with `/usr/bin/time`. A second wrapper runs rustup's rustc with the same flags, for the comparison.

The three `rust-toolchain.toml` files now say `channel = "1.100.0"` instead of `"stable"`. "stable" would move under the byte-for-byte comparisons every six weeks. rustup installed 1.100.0 as a toolchain of its own, the first time cargo ran.

`docs/era-9-rustc.md` has these findings in a new section, and they change the plan: no `ld.so`, LLVM 23 instead of 18, the libc list, and the memory numbers.

## R1: how it fits together

```
 program                                     kernel
 ──────────────────────────────────          ─────────────────────────────────────────
 p = mmap(1 GiB) ─► MEMORY_RESERVE ───────►  a region in the mapping area,
                                               Mapping::Reserved { writable }
                                               nothing in the page tables
 p[0] = 1   ──── page fault, not present ──►  fault_in(address, write):
                                               reserved? not filled meanwhile?
                                               account.charge(1)?   ── no ──► killed:
                                               zeroed frame, mapped              "out of memory -
                                               USER|OWNED|NX|WRITABLE            64 MiB used, its
            ◄─── the instruction runs again                                      limit is 64 MiB"
 read(fd, p + 512K, 5000)                     copy_to_user → user_pages: a page not
                                               yet touched is filled the same way
 munmap(p) ─► MEMORY_UNMAP ───────────────►  unmap every page, note the frames,
                                               shootdown(root) on x86, then free
                                               the frames and give back the charge
```

**Reserved memory** (`process.rs`, TheKernel):
- A process's mapping area held memory objects only. Now each region is a `Mapping`: either `Object(Arc<MemoryObject>)`, mapped whole as before, or `Reserved { writable }`.
- **MEMORY_RESERVE (31)** `(size, writable)` reserves up to 64 GiB of address space (`RESERVE_MAX`) and takes no memory.
- **`fault_in`** is called by the page fault handlers:
  - x86: error code bit 0 clear means "not present"; instruction fetches are not filled;
  - AArch64: a data abort from a program with cause `NotMapped`.

  It takes the process's two locks (mappings, then tables) and finds the region (`Regions::find`, a binary search). If another thread filled the page meanwhile, it returns `Filled` at once. Otherwise it charges one page to the account, allocates a zeroed frame and maps it OWNED, so destroying the tables frees it like any of the process's own pages. The handler returns and the instruction runs again. On AArch64, `commit_mappings` (`dsb ishst; isb`) makes sure the CPU sees the new entry first.
- **System calls that read or write a program's memory** go through `user_pages`. It now holds both locks and fills an untouched reserved page as the program's own access would have. If the limit is reached there, the call fails with `OutOfMemory` and the program isn't ended: the caller may be holding locks and references that ending it right there would never release.
- **The main stack** is reserved too, between `USER_STACK_TOP - stack` and the top. 8 MiB unless the program's PT_GNU_STACK asks for something else (64 KiB to 1 GiB, `elf.rs`). Nothing is mapped at load time any more.
- **Every region has an unmapped page on both sides.** `Regions` already left one after each region; the page before a region is the previous region's guard. For the first region, the program must now end one page below the mapping area (`elf::parse(file, MAPPING_AREA.start - PAGE_SIZE)`). A stack in a region overflows into a fault.

**The account** (`account.rs`, new, host-tested):
- `Account { used, limit }`, in pages, lock-free: `charge` is a compare-and-swap loop that fails rather than go over the limit. A page fault on one CPU and a system call on another may charge the same account at once.
- What is counted:
  - the pages a program's file is loaded into;
  - every reserved page as it is filled;
  - every memory object the program makes with MEMORY_CREATE, for as long as the object lives, wherever its handle went. The object holds an `Arc<Account>` and gives the pages back in its `Drop`.

  Page tables and the kernel's own records are not counted.
- **The default limit** is the memory free when the kernel had set itself up, minus a reserve of a sixteenth (at least 16 MiB) that no single program can take. With 4 GB that is 3836 MiB on x86 and 3837 on AArch64; with 512 MB, 476.
- **MEMORY_LIMIT (32)** lowers the limit, never raises it; 0 only asks. **MEMORY_USED (33)** says what is counted now.
- **A new program's limit** is the lowest of three:
  - the default;
  - the limit of the process that loads it;
  - the limit of whoever made the memory object its file came in.

  The third is the one that matters: `posix_spawn` and the shell ask *init* to start a program. If the limit came only from the loader, every program would get init's. But the one asking read the file into an object of its own, which is counted against it and can't be counted against anyone else.

**Programs and libraries:**
- `PROGRAM_IMAGE_MAX` and `MEMORY_MAX` (one memory object) are 512 MiB, `elf::MEMORY_MAX` (a program's segments) 1 GiB. The real bound is now the account.
- The `taaj` crate has `memory_reserve`, `memory_limit` and `memory_used`.
- **The Rust heap** (`user/src/allocator.rs`) grows by reserving regions (1 MiB to 4 GiB, doubling) instead of making memory objects. A program that reserves a lot and uses a little takes a little. As on Linux with overcommit, running out shows when a page is touched, not at `malloc`. Since libc's malloc is this heap, C programs get the same.
- **Thread stacks**, C's and Rust's, are reserved: **8 MiB by default for pthreads**, as glibc gives, and 2 MiB for `taaj::thread`, as Rust's std gives. The unmapped page under each one is the guard entry 095 said was missing.
- **`mmap`** (`libc/c/mmap.c`) reserves instead of copying into the heap. Anonymous memory costs what is touched, a file is read into the mapping, and `munmap` gives every page back. Its list of mappings now has a lock, as threads may map at once.
- **`getrlimit`/`setrlimit`** with `RLIMIT_AS` or `RLIMIT_DATA` read and lower the kernel's limit. Raising it is EPERM.

**The TLB shootdown** (`tlb.rs`, new):
- Each CPU keeps the translations it has used in its TLB and goes on using them after the page table entry is gone, until told to forget. With one thread per process, the only CPU to tell was the one doing the unmapping. Since entry 095 another thread of the same program may be running on another CPU, still able to read and write a page whose frame is about to be freed and given to someone else.
- On **AArch64** that was never a problem: `tlbi vaae1is` broadcasts to every CPU, and the `dsb` after it waits for all of them.
- On **x86**, `invlpg` is for one CPU only. So `unmap_memory` now:
  1. takes the entries out under the locks;
  2. lets the locks go and calls `shootdown(root)`, which marks every other CPU whose CR3 holds this process's tables (`threads::cpus_running`, from `ACTIVE_ROOT`) and sends each the new vector 0xF1;
  3. waits until every mark is cleared; each CPU clears its mark and then reloads CR3;
  4. only then frees the frames and gives back the charge.
- The sender waits with interrupts off, so it can't move to another CPU meanwhile, and holds no lock. Two CPUs shooting at each other at once don't deadlock: each answers its own mark while it waits.

**The test** (`bigmem.c` and `bigmem-test.sh`, run by `make bigmem64-test` and `make aarch64-bigmem-test`) boots with 4 GB and 4 CPUs. Below, under "How I checked it".

## Design decisions

| Decision | Chosen | Rejected, and why |
|---|---|---|
| How memory is given lazily | Reserved regions filled on a page fault | Making memory objects lazy: an object is shared between processes, and filling it on a fault in one would need the others' tables too. Reserved memory is one process's own, which is what stacks, heaps and anonymous `mmap` are |
| Running out at a page fault | End the program, with its use and limit in the log | Returning an error: there is no one to return it to. The program was just writing a byte. Linux sends SIGKILL (the OOM killer) or SIGSEGV here |
| Running out inside a system call | The call fails with `OutOfMemory` | Ending the program there: the call's callers may hold locks and an `Arc` of the process, which `end_current` (it never returns) would never let go |
| Who a new program's limit comes from | The lowest of: the default, the loader's, the maker of the file's object | A limit field in the spawn request: init would pass on whatever the asker claimed, and a program could ask init for more than its own |
| The default limit | What was free at boot, less a sixteenth (16 MiB at least) | No limit unless asked: one runaway program takes every frame, and the services fail with it. `total_frames()`: that is the span of the bitmap, 5 GiB on a PC with 4 (see Bugs) |
| Shootdown | Reload CR3 on the other CPUs, for the whole process | `invlpg` for each page there: the list of pages would have to travel to every CPU, for a saving that matters only with many small unmaps |
| Who is told | CPUs whose `ACTIVE_ROOT` is the process's tables | Every CPU: correct, but an interrupt for each unmap on every idle CPU |
| `mprotect` / `PROT_NONE` | Not enforced: reserved memory is always readable and writable | Enforcing it means splitting regions and changing page flags later: needed for guard pages made by the program itself, not yet by anyone |
| Main stack | Reserved, 8 MiB by default, up to 1 GiB | Mapped whole at start (as before): 8 MiB per program, used or not |

## Bugs

- **The default limit was 4800 MiB on a 4 GB machine.** I took `total_frames()` for the machine's memory. It is the number of frames the bitmap covers: from 0 to the highest address, 5 GiB on a PC, whose memory above 3 GiB moves above the devices at 4 GiB. It is now the free count at the end of `mm::init`, `boot_free_frames()`.
- **A lowered limit was not inherited.** `bigmem limit` lowered its limit to 64 MiB and started `bigmem report`, which said 3836. The kernel took the limit from the process loading the program, and that is init: `posix_spawn` asks init to start programs. Now it also takes the limit of whoever made the file's memory object.
- **The overflow test came back instead of faulting, twice:**
  - First, each 64 KiB frame wrote its lowest byte first. The stack pointer had already jumped 64 KiB down, over the one unmapped page, into a region below it that was perfectly writable. That's the classic "stack clash", and it is why Rust probes every page of a big frame from the top and clang has `-fstack-clash-protection`. The test now touches each page top down, as real code uses a stack.
  - It still came back, and the disassembly showed why: **clang had shrunk `volatile char frame[64 * 1024]` to 32 bytes**. Only 17 of its bytes were ever accessed and its address never escaped, so the optimizer (SROA) kept just those. Every "deep" call used 48 bytes of stack. So "48 MiB deep on a 64 MiB stack" had passed without going deep at all. An empty `asm volatile` that takes the array's address makes the compiler keep all of it. Now the frames are 64 KiB (`sub $0x10000,%rsp`), the 48 MiB check is real, and the overflow is killed by a page fault one page under the stack.
- **`read()` brought fewer bytes than asked**, and my first check expected 5000 at once. A read may return less. The test now loops, as every reader must; `mmap.c` already did.
- **rustc refused rustup's std** (E0514) until `build.description = ""` (R0 above).
- **A mutation outlived its experiment.** The first `make test64` failed in `smp64-test`: heapcheck got `BadAddress` receiving into reserved pages, exactly what the last mutation did. My script put each file back with `mv` from a copy taken before the change. `mv` keeps the copy's time, older than the mutated build, so cargo took the mutated kernel for up to date and kept it. Touching the files rebuilt it. The script must `touch` what it restores.

## How I checked it

**Host tests**: `cargo test` in kernel64 has 201 tests, up from 194:
- `Regions::find`: a region's first and last byte, its guard page, the area's edges, a removed region;
- `Account`: charges stop exactly at the limit; a refused charge takes nothing; the limit only goes down; the default keeps its reserve;
- the ELF stack sizes: the 8 MiB default, 64 MiB asked, 1 GiB the most.

**New boot test, `make bigmem64-test` and `make aarch64-bigmem-test`**: 4 GB, 4 CPUs. TheTaaJ runs:
- `with files bigmem`, **23 checks**:
  - 1 GiB mapped takes under 1 MiB; touching 64 MiB of it takes 64 to 66 MiB; every touched page keeps its byte, an untouched one reads 0; `munmap` gives it all back;
  - a 10,000-byte file mapped, with zeros after its end;
  - `read()` into a mapping nobody has touched, across two pages;
  - 6 MiB deep on the main stack, and 48 MiB deep on a thread's 64 MiB stack;
  - 60 threads at once with 8 MiB stacks each, 480 MiB reserved: **184 KiB taken** (x86), 300 KiB (AArch64);
  - `malloc(512 MiB)` with 100 MiB of it written takes 100 to 110 MiB.
- `bigmem big`: **3072 MiB touched in 1.9 s (x86) and 2.7 s (AArch64)**, under QEMU's emulation (this host has no KVM). The account rose by 3072 MiB, every page kept its word, and after `munmap` 0 MiB was left.
- `with programs bigmem limit`: the limit is lowered to 64 MiB and raising it again is refused. The program it starts says "my limit is 64 MiB". Touching 256 MiB ends it: `killed: out of memory - 64 MiB used, its limit is 64 MiB`. The shell, and the next program, carry on.
- `bigmem overflow`: a thread with 1 MiB of stack goes 2 MiB deep and is killed by a page fault at the page under its stack.
- `bigmem shoot`: one thread reads a page in a loop while main unmaps it from another CPU. The reader is killed at once, by a page fault at that page.
- `bigmem report`, at the end: the next program has the machine's limit again.
- Every boot also runs the kernel's self-tests, which check that no frame is lost after programs end. The lazily filled pages of the heap and stacks pass them. `heapcheck`, which the self-test runs twice, now also receives a message straight into reserved pages nobody has touched (see the mutations below).

**Mutation experiments**, each undone:

| What I broke | What happened |
|---|---|
| `tlb::shootdown` returns at once (no shootdown on x86) | Caught: `bigmem shoot`'s reader went on reading the unmapped page for a whole second, on its CPU's stale translation, and the program printed "not reached" |
| A filled page is not charged | Caught: the 23 checks failed (touching 64 MiB "took" nothing) |
| `munmap` of reserved memory doesn't give the charge back | Caught: the checks failed ("munmap gave every page back") |
| `user_pages` doesn't fill an untouched page (a system call gets `BadAddress`) | **Not caught at first**: every C library path copies through its own buffers, so the kernel never wrote into an untouched page in any test. I added a check to `heapcheck`, the Rust program the kernel's self-test runs at every boot: a channel message received straight into two reserved, untouched pages, the two pages counted, and given back. With it: heapcheck panics at the receive, and the self-test stops the boot |
| A new program's limit doesn't come from the file's object | Caught: "the program bigmem started did not get its lowered limit" |

`make test64` passed on the second run, in 14 min 45 s, with the two new parts. The first run failed in `smp64-test` on a kernel left over from the last mutation (see Bugs). I also ran the testing note's steps below, typed by script into `make run64 RUN_MEMORY=4G` on a scratch disk made by `make run-disk`: every row as the table says (`bigmem big` 1.6 s there; `threads speed` 4.2 times as fast).

## What is still missing

- **A program's file is still loaded whole**, and while it loads the file is in memory twice: in its memory object and in the program's pages. Demand paging of the file itself (pages read from the object when first touched) is the "better" half of R1's plan. It isn't needed until a 200 MB rustc: TheTaaJ has 4 GB now.
- **`mprotect` and `PROT_NONE` do nothing**, `MAP_SHARED` of a file can't write back, `MAP_FIXED` is refused, and `munmap` of part of a mapping leaves it whole.
- **No giving memory back inside a reservation** (`madvise(MADV_DONTNEED)`): a heap that shrinks keeps its pages until the program ends.
- **Page tables are not counted** against the limit, nor are kernel stacks of threads.
- **A race older than this step**: `user_pages` returns physical addresses and lets the locks go before the copy. Another thread of the same program can unmap the memory meanwhile, and the copy then touches a frame no longer the program's. It takes a program racing against itself to do it, but it is a hole. Closing it means holding the frames, or the lock, for the copy. It is written down in `user_pages`.
- **C has no stack probes**: a C function with a frame bigger than a page can step over a guard page (the bug above). Rust programs probe.
- **R0's remaining questions** go to the steps that need them: the `anon.<hash>` difference in bitcode (R6, if it matters), LLVM 23 for TheTaaJ (R4), and the libc functions (R3).

## What I learned

- **Lazy memory moves the failure.** With pages taken when touched, `malloc` almost never fails. The program runs out at some ordinary store instruction, and the only honest answer there is to end it and say why. That's what Linux's overcommit and its OOM killer are about.
- **A TLB is a cache nobody tells you about.** The page table is the truth, but each CPU acts on its own copy. Unmapping is three steps: take the entry out, make *every* CPU forget it, and only then free the frame. Skip the middle step and a thread on another CPU goes on writing into memory that now belongs to someone else.
- **"The kernel's caller" isn't always the program that asked.** posix_spawn goes through init, so "the parent" is init. The limit has to follow something the asker owns, here the memory object it read the file into.
- **The compiler is allowed to make a test meaningless.** A `volatile` array whose address never escapes may be shrunk to the bytes used. I read the disassembly before trusting a test about stack depth, and should do it for any test about how much memory something takes.

**Rust, as used here:**
- **An enum that carries data**, `Mapping::Object(Arc<MemoryObject>)` or `Mapping::Reserved { writable }`. One `Regions<Mapping>` holds both kinds, and `match` makes every place that looks at a region handle both.
- **A value held only so it gets dropped.** `Object`'s `Arc` is never read; it keeps the object, and so its frames, alive while mapped, and dropping it is what frees them. The compiler warns about a field nobody reads, so it carries `#[allow(dead_code)]` with a comment saying why.
- **`AtomicU64::try_update`** (formerly `fetch_update`) takes a closure from the current value to the next, or `None` to refuse. It retries the compare-and-swap until no other CPU got in between. `used.checked_add(pages).filter(|&total| total <= limit)` is the whole charge rule: overflow and the limit both give `None`.
- **`fetch_min`** lowers a limit atomically: the new value is the smaller of the old and the given one, so two lowerings at once can't raise it.
- **`slice::partition_point`** is binary search by a predicate. On a sorted list of regions, `partition_point(|r| r.start <= address) - 1` is the only region that can contain the address.
- **`Option<Arc<Account>>`** on a memory object: only objects made for a program are charged; device memory and the kernel's own copies are `None`. `Drop` gives back exactly what was charged, however the object ends.
- **`#[cfg(target_arch = "x86_64")]` on a whole module**, with an AArch64 function of the same name that does nothing. Code outside calls `tlb::shootdown` with no `cfg` of its own.

## Testing note

**1. The PC**

```
make run-disk
make run64 RUN_MEMORY=4G
```

Close QEMU before `make run-disk`. It puts `bigmem` on the disk. `RUN_MEMORY` is new; it defaults to 512M as before.

| Type | You should see |
|---|---|
| `with files bigmem` | `bigmem: limit 3836 MiB, 0 MiB used at the start`, then `bigmem: 60 threads, 480 MiB of stacks, took N KiB` (a few hundred) and `bigmem: 23 checks, all passed` |
| `bigmem big` | after a few seconds, `bigmem: touched 3072 MiB in N s; it took 3072 MiB; every page kept its word: yes; after munmap: 0 MiB` |
| `with programs bigmem limit` | `bigmem: limit lowered to 64 MiB, raising it refused`, `bigmem: my limit is 64 MiB`, `bigmem: 256 MiB mapped: yes; touching it all`, then `bigmem was killed by the kernel` and, on the serial console, `killed: out of memory - 64 MiB used, its limit is 64 MiB` |
| `bigmem overflow` | `bigmem: a thread with 1 MiB of stack goes 2 MiB deep`, then `bigmem was killed by the kernel` (a page fault under its stack) |
| `bigmem shoot` | `bigmem: unmapping the page another thread is reading`, then `bigmem was killed by the kernel` at once |
| `bigmem report` | `bigmem: my limit is 3836 MiB` |
| `threads speed` | as in entry 095: threads still work, now on 8 MiB stacks |

With `make run64` (512 MB), `bigmem` says a limit of 476 MiB, and `bigmem big` touches what the limit allows.

**2. AArch64**

```
make run-aarch64-disk
make run-aarch64 RUN_MEMORY=4G
```

The same as part 1, the limit 3837 MiB.

**3. The tests by themselves**

```
make bigmem64-test
make aarch64-bigmem-test
make test64
```

| Look for | Meaning |
|---|---|
| `a bigger machine (x86_64, 4 GB): 23 checks of reserved memory, mmap and big stacks passed; ...` | every part above; the same for aarch64 |
| `test64: all passed` | every part, the new two included |

**4. R0, on the host (optional, takes about 20 minutes and 15 GB)**: `build/r0/` holds the rustc source and the scripts, outside git. With cmake and ninja on the PATH, `./x.py build --stage 1 compiler/rustc` in `build/r0/rustc-1.100.0-src`; then `build/r0/measure-all.sh` builds our crates with it and writes each crate's memory and time to `build/r0/measure-*.log`.

Automated: make bigmem64-test, make aarch64-bigmem-test, make test64
