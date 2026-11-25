# Era 9 · Rust at home: the real rustc on TheTaaJ, and TheTaaJ built by itself

*A plan, not a journal entry. Written 2026-11-25, after Era 6's last planned step (entry 094). Era 6 ended with TheTaaJ building its C library and C programs, but its milestone, "TheTaaJ builds TheTaaJ", was not reached: most of TheTaaJ is Rust, and there is no Rust compiler on it. This era finishes that milestone with the real `rustc`, not a substitute.*

## Done when

On TheTaaJ, from the sources on its own disk, one `make`:
- builds the kernel, every Rust program, both halves of the C library and both bootloaders, for x86_64 and AArch64;
- writes a bootable image;

and that image boots (in QEMU, started from the host) and passes `make test64`. Wherever TheTaaJ and the host use the same compiler with the same flags, the output is byte for byte the host's.

## Where we start

What is already there, and helps:
- **LLVM runs on TheTaaJ** (entries 073–076): llc, clang and lld, all byte for byte as the host's. rustc's code generator *is* LLVM, so the biggest dependency has been ported once already.
- **A POSIX-shaped C library**: files, directories, `posix_spawn`, pipes, the environment, time, stdio. C++ programs run (entry 072).
- **make (pdpmake) and sh** run there, with redirections, pipes and here-documents (entries 078–082, 092).
- **Our Rust code is simple to build**: 19 crates, all on stable Rust, on the built-in bare-metal targets (`x86_64-unknown-none`, `aarch64-unknown-none-softfloat`), with **no external crates** and only two small build scripts. Nothing has to be downloaded or vendored.
- **Faster writes and long names** (entries 093, 094): a compiler's output goes to the disk at a megabyte per commit, and every rustc source and libc++ header name fits.

What is missing (measured on the host, 2026-11-25, rustc 1.100.0):

| Need | rustc's side | TheTaaJ today |
|---|---|---|
| Its size | `librustc_driver.so` is 164 MB, and LLVM is linked in. A static rustc would be about 200 MB | A program file may be at most 128 MiB (`PROGRAM_IMAGE_MAX`) |
| Memory | Rebuilding the kernel peaked at 424 MB (cargo, release, on the host) | QEMU runs it with 512 MB in all |
| Threads | rustc runs the compiler on a spawned thread with a big stack, and uses thread-locals throughout | One thread per program; `pthread_create` fails with EAGAIN |
| Shared libraries | rustc is a small `rustc` binary plus `librustc_driver.so` | No dynamic loader |
| Rust's `std` | rustc is an ordinary std program | No std target for TheTaaJ |
| Memory-mapped files | rustc maps crate metadata (`.rlib`, `.rmeta`) into memory | Memory objects exist; no `mmap` of a file |
| A build driver | Our build uses cargo | No cargo; cargo also links curl, git and TLS |
| The BIOS bootloader | `nasm` and `i686-elf-gcc` | Neither runs on TheTaaJ |
| The image | `xorriso` writes the ISO | No ISO writer |

## The steps

Grouped into a few big steps, each with its own journal entry, tests and testing note. Each one is useful on its own, not only for rustc.

### R0 · Measure first (host only)

Build rustc 1.100.0 from source on the host, with the same LLVM, and answer the questions the later steps depend on:
- **Can rustc be linked statically**, `rustc_driver` included? If yes, R4 needs no dynamic loader. If not, we write a small one (`ld.so`: `DT_NEEDED`, relocations, TLS).
- **Peak memory and time** for each of our crates, single-threaded (`-C codegen-units=1`, `-Z threads=1`, LLVM without threads). This sets how much memory R1 must give, and how long the milestone build will take.
- **Which parts of std rustc actually uses**: threads, `mmap`, `Command`, environment variables, temporary files, signals. That's the list R2 and R3 must cover.
- **Pin the toolchain**: `rust-toolchain.toml` says `channel = "stable"` today. For byte-for-byte comparisons, the host and TheTaaJ need exactly the same rustc, so it becomes `1.100.0`, and rises only on purpose.

Output: a short report in the journal, and the plan below corrected where the numbers say so.

### R1 · A bigger machine

- **Memory**: QEMU with 2–4 GB, and the kernel using all of it on both architectures: the frame allocator, the direct map, and AArch64's page tables beyond what is used now.
- **Big programs**: `PROGRAM_IMAGE_MAX` and `MEMORY_MAX` from 128 MiB to at least 512 MiB. Better: a program's pages are read from its file when first touched (demand paging), instead of the whole file being loaded up front.
- **`mmap` for files and big anonymous memory** in libc, over memory objects: rustc maps metadata, and the allocator wants big blocks it can give back.
- **Per-process memory limits**, so one big compile can't starve the services. The `MEMORY_MAX` comment has been promising them since entry 041.
- **Big stacks**: 64 MB and more on request.

### R2 · Threads in a program

The kernel has had threads on every CPU since entry 046; programs get them now.
- A process may have many threads, each with its own stack, and **thread-local storage**: the FS base on x86_64, `TPIDR_EL0` on AArch64, and the ELF TLS layout clang and rustc expect.
- A **wait-on-address** system call (a futex), so mutexes and condition variables sleep in the kernel instead of spinning.
- libc's **pthreads** for real: create, join, detach, mutex, cond, rwlock, keys, once. `std::thread` in our C++ programs starts working too.
- Tests: many threads in one program on 4 CPUs, a lock under contention, TLS seen from each thread, a thread that exits while others wait.

### R3 · Rust's std on TheTaaJ

- New targets `x86_64-unknown-taaj` and `aarch64-unknown-taaj`, in `ports/rust` (Rust's sources at 1.100.0, with our changes kept as a small set of patches).
- **std's backend over TheTaaJ's own libc**: `target_family = "unix"`, `target_os = "taaj"`, the way Rust supports other Unix-like systems. This reuses everything libc already does, and finds what it doesn't: `clock_gettime`, `getrandom`, `realpath`, `sigaltstack`, `dl_iterate_phdr` and others. Each missing one is added to libc or answered honestly as unsupported.
- `panic = "abort"` at first; unwinding later, if rustc needs it.
- **Done when**: a Rust program using std (files, directories, environment variables, `process::Command`, threads, time) runs on both architectures, and a chosen subset of std's own tests passes there.
- A side effect: Rust programs for TheTaaJ can use std from now on, not only `core` and our `taaj` crate.

### R4 · rustc runs on TheTaaJ

- Cross-build rustc on the host for the `*-unknown-taaj` host targets with Rust's own bootstrap (`x.py`), linked against LLVM statically, and statically or through R0's loader.
- Also its **sysroot** for our two bare-metal targets: `core`, `alloc` and `compiler_builtins`, built by that same bootstrap so the versions match exactly.
- Put on the disk: `/bin/rustc` and `/lib/rustlib/...`.
- **Done when**: on TheTaaJ, `rustc --target x86_64-unknown-none` compiles our `taajfs` crate to an `.rlib`, and then a small program, **byte for byte as the host's rustc 1.100.0** with the same flags (`--remap-path-prefix` keeps paths out of the output). The same on AArch64.

### R5 · One recipe for building our Rust code, without cargo

Cargo is big, and much of it is curl, git and TLS, which we don't need: there are no external crates. So:
- A **Makefile that drives rustc directly**, crate by crate in dependency order: `--crate-type`, `--extern`, `--edition`, the same `-C` flags cargo passes. It covers all 19 crates and every program, on both architectures.
- The two `build.rs` scripts become flags (the linker script) and a rule (the big-image test's ballast file).
- It runs **on the host as well, and must produce the same bytes as cargo there**. So the host checks the recipe every time it builds, and TheTaaJ can't drift from it.
- **The bootloader**: `nasm` is ported (portable C, a few thousand lines), and the BIOS half moves from `i686-elf-gcc` to clang on the host too (`--target=i686-elf`). Then both sides use the same compiler, and their outputs can be compared byte for byte. The UEFI half already uses clang and lld.
- Cargo on TheTaaJ is left for later, possibly built without its network parts.

### R6 · The milestone: TheTaaJ builds TheTaaJ

- `make` in `/src` on TheTaaJ, with the tree put there by an extended `tools/put-sources.sh`, builds:
  - the kernel for both architectures;
  - every Rust program and service;
  - libc, both halves (the Rust half no longer arrives ready-made as `/lib/libc-rust.o`);
  - both bootloaders.
- **The image**: a small ISO 9660 / El Torito writer of our own (a Rust program; the format is small) instead of porting xorriso. It writes the same hybrid image `make iso64` makes.
- The host takes the image TheTaaJ wrote, boots it, and runs `make test64` against it.
- Then the circle closes: the system built on TheTaaJ builds TheTaaJ again, and the result is the same bytes.

## Decisions taken in this plan, and why

| Decision | Chosen | Rejected, and why |
|---|---|---|
| Which compiler | The real rustc | mrustc (Rust to C): it supports only older Rust versions and doesn't check borrows. It would build *a* TheTaaJ, not show that ours builds as the host builds it |
| Where std's backend sits | Over TheTaaJ's libc (a Unix-like target) | Straight over system calls: std's Unix code is the most used and tested, and libc already exists; a backend of our own can come later, file by file |
| How rustc gets there first | Cross-built on the host by Rust's bootstrap | Built on TheTaaJ from the start: compiling rustc takes hours and many GB even on the host. Rebuilding rustc *on* TheTaaJ is the next era's question, not this one's |
| The build driver | A Makefile calling rustc, checked against cargo on the host | Porting cargo first: weeks for curl/git/TLS that nothing here needs |
| The image | Our own ISO writer | Porting xorriso: large, and we use one corner of it |

## Risks

- **Time.** In QEMU, with one compiler thread, the kernel will take minutes, not the host's second or two. R0 measures this; if it's too slow, `-C codegen-units` above 1 needs the threads from R2 anyway.
- **Memory.** If rustc needs more than R1 can give inside QEMU, the milestone build may need `opt-level` or `codegen-units` choices the host doesn't make. Then the byte-for-byte comparison is made with the same flags on both sides, not with today's.
- **rustc's assumptions about its OS.** Jobserver pipes, signals, `/proc`, `getrlimit` for the stack. R3's std tests and R4's first runs will find them. Each one is either answered properly or stubbed honestly and listed.
- **Version churn.** Pinning 1.100.0 is a decision to stay; raising it later means rebuilding `ports/rust` and checking the patches.

## Not in this era

- rustc compiling rustc on TheTaaJ (full compiler self-hosting).
- cargo on TheTaaJ, and crates from crates.io.
- Incremental compilation, rust-analyzer, rustdoc.
- A real ISO writer for other people's needs.

## Where it fits

- Era 6's milestone is this era's milestone. Era 6 stays marked "planned steps done, milestone not reached" until R6.
- It doesn't depend on Era 7's last step (R7, the real PC) or on Era 8 (the AI assistant, still to be discussed). Which comes first is the user's choice.
- R1 (memory, `mmap`, demand paging) and R2 (threads) are exactly what Era 8's inference service would need too, so they aren't wasted whichever order is chosen.
