# LLVM on TheTaaJ

*Journal entry #73 · Era 6, step E1. The first piece of LLVM runs on TheTaaJ: `llvm-tblgen`, the program LLVM's own build runs before anything else. It is built from LLVM 18.1.3's sources, imported into `ports/llvm/llvm`. It reads the 25 `.td` files that describe every target's intrinsics, and writes the same 3.2 MB of C++ that the host's `llvm-tblgen` writes, byte for byte, on both machines. To get there the C library grew most of the rest of POSIX that LLVM's Support library uses: `stat`, directories, `pthread` for one thread, signals a process sends itself, `mmap` by copying, `sysconf`, and `thread_local` for C++. `malloc` also got size classes for small blocks, which made the intrinsics job 20 times faster.*

## Why

The goal of Era 6 is a C compiler on TheTaaJ that is not TCC: Clang. Clang is LLVM, and LLVM is about three million lines of C++ resting on one library, Support, which wraps the operating system: files, memory, processes, time, signals, threads. Nothing else in LLVM talks to the system directly. So the first step is to make Support work. The smallest program that proves it does is `llvm-tblgen`.

TableGen is LLVM's own little language for describing machines: registers, instructions, intrinsics, value types. LLVM's build runs `llvm-tblgen` over those `.td` files to write `.inc` files of C++, which the rest of LLVM then includes. It is a real program: it reads many files, parses a language, builds a large in-memory model, and writes megabytes. Yet it needs only Support and TableGen's own two libraries, about 230 files. If it writes exactly what the host's copy writes, Support works on TheTaaJ for everything it was asked to do.

## How it fits together

```
 ports/llvm/llvm/              LLVM 18.1.3, as upstream (include, lib, utils/TableGen)
   + four small patches        marked "TheTaaJ:" in the source
 ports/llvm/taaj/
   llvm-config.txt             what CMake would have found out about the system
 tools/build-llvm.py           reads LLVM's CMakeLists.txt for the lists of sources,
                               writes the configured headers, runs the HOST's
                               llvm-tblgen for the .inc files LLVM needs while it is
                               being built, compiles, archives and links
          |
          v
 build/llvm/x86_64/bin/llvm-tblgen   (3.6 MB; 3.2 MB for AArch64)
          |   linked with libc++.a (now with threads) and libc.a
          v
 TheTaaJ:  with files llvm-tblgen -gen-intrinsic-impl -I src/llvm/include \
                src/llvm/include/llvm/IR/Intrinsics.td -o impl.inc
```

**The build without CMake.** LLVM's build system is CMake, and CMake begins by trying the compiler and the C library: does `pthread.h` exist, does `sysconf` link, is there `st_mtim` in `struct stat`. Teaching CMake that TheTaaJ is a system would be harder than doing its job. `build-llvm.py` (365 lines) does that job for this one configuration:
- It reads `add_llvm_component_library(...)` and `add_tablegen(...)` out of the real `CMakeLists.txt`, so the lists of sources stay upstream's.
- It fills in the `.h.cmake` templates (`#cmakedefine`, `${...}`) from `llvm-config.txt`, where each answer CMake would have found is written down by hand.
- It runs `tablegen(LLVM ...)` jobs with the host's `llvm-tblgen` of the same release.
- It compiles in parallel, rebuilding only what changed (from clang's `-MD` dependency files).
- It archives each library and links the tool.

**The C library's new half.** LLVM's `Unix/*.inc` files are written for any POSIX system. Each missing function or header was a compile error or an undefined symbol, so the list of what to add wrote itself:

| What | Where | How much of it |
|---|---|---|
| `stat`, `fstat`, `lstat`, `mkdir`, `rmdir`, `access`, `chdir`, `truncate` | `status.rs` | a file's kind and size; owners, permissions and times are not kept, so they are 0 |
| `opendir`, `readdir`, `closedir` | `status.rs` | `.` and `..` first, then the directory's names |
| `st_ino` | `status.rs` | a 64-bit hash (FNV-1a) of the whole path |
| `dup`, `dup2`, `fsync`, `fcntl` | `descriptors.rs` | `F_GETFL`, `F_SETFL`, `F_GETFD`, `F_SETFD`, `F_DUPFD`; locks fail with `ENOLCK` |
| `pthread_mutex_*`, `_cond_*`, `_rwlock_*`, `_once`, `_key_*` | `pthread.c` | for a process with one thread (see below) |
| `__emutls_get_address` | `emutls.c` | C++'s `thread_local` |
| `signal`, `sigaction`, `raise`, `kill`, `sigprocmask` | `signal.c` | signals a process sends itself |
| `mmap`, `munmap` | `mmap.c` | the file is read into new memory |
| `getuid` and friends, `getpwuid_r`, `statvfs` | `users.c` | there are no users: everyone is 0 |
| `nanosleep`, `sysconf`, `getrlimit`, `getpid`, `gethostname` | `system.rs` | |
| `fork`, `execve`, `waitpid` | `system.rs` | fail with `ENOSYS` / `ECHILD` |
| `strerror_r`, `strsignal`, `rand` | `errno.rs`, `signal.c`, `stdlib.c` | |

**One thread, honestly.** TheTaaJ processes have one thread each. LLVM can be built without threads (`LLVM_ENABLE_THREADS=0`), but libc++'s `<mutex>` and libc++abi still want a thread API. So `pthread.c` implements one that is correct for exactly one thread:
- A mutex is a counter. Locking a held one can only mean the caller holds it, because no one else exists. So a normal mutex reports `EDEADLK` instead of waiting forever, and a recursive one counts.
- `pthread_cond_wait` returns `EDEADLK`: waiting for a signal that only this thread could send is a deadlock. `pthread_cond_timedwait` sleeps until its deadline and says `ETIMEDOUT`.
- `pthread_create` says `EAGAIN`, "no resources for another thread", which is the truth. `std::thread` turns that into an exception.

libc++ is now built with its pthread layer instead of `_LIBCPP_HAS_NO_THREADS`, so `std::mutex`, `std::call_once`, `thread_local` and condition variables work in C++ programs.

**`thread_local` without threads.** A `thread_local` variable normally lives at an offset from a thread pointer register (`fs` on x86-64, `TPIDR_EL0` on AArch64). TheTaaJ's kernel does not set one up for programs yet. `-femulated-tls` makes the compiler call `__emutls_get_address(&control)` instead. The control record holds the variable's size, alignment and first value; the first call makes a copy and later calls return it. One thread means one copy.

## Design decisions

| Decision | Chosen | Rejected, and why |
|---|---|---|
| Building LLVM | `tools/build-llvm.py`, reading upstream's `CMakeLists.txt` for the lists | CMake: its system checks run test programs on the host, and a cross build for an unknown OS needs a toolchain file and dozens of cache answers anyway. A hand-written file list: it would go stale on the next LLVM update |
| The `.inc` files LLVM needs to build itself | The host's `llvm-tblgen` 18 writes them | Building a host `llvm-tblgen` from `ports/llvm` first: a second whole build, for the same output (the test shows it is the same) |
| Which part of LLVM to import | `include`, `lib` with only the X86 and AArch64 targets, `utils/TableGen` (98 MB) | All of `llvm/`: 780 MB of tests, and every other target |
| Threads | `LLVM_ENABLE_THREADS=0`, and a pthread for one thread | Threads in the kernel first: a large job, and not needed to compile one file |
| `thread_local` | `-femulated-tls` | Real TLS: needs the kernel to set `fs`/`TPIDR_EL0` per process and the loader to lay out the TLS block. Later, with threads |
| `st_ino` | FNV-1a hash of the whole path | A field in TaajFS: a disk format change. 0 for everything: clang's `FileManager` uses `(st_dev, st_ino)` to know two names are one file, and would treat every file as the same one |
| A mutex locked twice | `EDEADLK` | Wait: with one thread, waiting is hanging forever. POSIX allows `EDEADLK` for error-checking mutexes; returning it for the default kind too makes the mistake visible |
| `mmap` | Read the file into new memory | Real mapping: there is no page cache or shared memory to map from yet. LLVM's `MemoryBuffer` only reads what it maps |
| Small `malloc` blocks | 16-byte size classes up to 1 KiB, each class a free list, cut from 64 KiB pieces | Keep one first-fit list: `llvm-tblgen` makes millions of small objects, and every `malloc` walked a list that kept growing |
| LLVM's warnings | `-w` | Fixing them: they are upstream's, and patches would make the next update harder |
| Patches to LLVM | Four, each marked `TheTaaJ:`, in a commit after the plain import | A fork: the import commit keeps upstream's files exactly as released |

The four patches: wide-string conversions in `ConvertUTF` are left out when libc++ has no wide characters; `LockFileManager` seeds its random engine from the clock (there is no `random_device`); `is_local` says every file is local; and `getMainExecutable` returns an empty path, because a program on TheTaaJ is not told its own name. That last one will matter for Clang, which finds its headers relative to itself.

## Bugs

- **`-gen-vt` was an unknown option.** TableGen's backends register themselves: each `.cpp` file has a static object whose constructor adds its option to a table. Nothing calls into those files directly. So when they were in an archive, the linker saw no reason to take them out, and the backends were simply absent. CMake has the same problem and solves it with an "object library". `build-llvm.py` now links `LLVMTableGenCommon`'s object files directly instead of through an archive.
- **The intrinsics job took 185 seconds.** The host's `llvm-tblgen` does the enums job in 0.36 s. The cause was the heap: `llvm-tblgen` makes millions of small objects, and every `malloc` walked one free list that kept growing. With size classes the job takes about 8 s on the PC. It is still slower than Linux, mostly because files are read 1 KiB per request.
- **`Intrinsics.td` itself was missing from the test disk.** The test puts on the disk every file the host's `-d` dependency list names. The list names every file the input *includes*, not the input itself.
- **Wrong names in the generated `.def` files.** `llvm/Config/AsmPrinters.def` and its siblings are filled from CMake lists with names like `LLVM_ENUM_ASM_PRINTERS`. I had guessed them from the file names and got them wrong. They are now written out explicitly.
- **`strerror_r` was missing**: libc++'s `system_error.cpp` uses it. Added, with a test of the case where the buffer is too small.
- **A false alarm in my own test.** The first version failed on any page fault in the log. The boot's own self-tests fault on purpose (`echod`, `peek`), so the test now checks only for `"llvm-tblgen" killed`.

## How I checked it

Automated, by booting: `llvm64-test` and `aarch64-llvm-test` (in `make test64`). Each makes a 32 MB disk. The host's `llvm-tblgen` runs the same three jobs first, and its `-d` lists say which of the `.td` files go on the disk (25 of them, 1.9 MB). Then TheTaaJ runs:

| Job | Writes | PC, alone | PC, in `make test64` | AArch64, alone |
|---|---|---|---|---|
| `--version` | `TheTaaJ LLVM version 18.1.3` | | | |
| `-gen-vt` | `vt.inc`, 18,018 bytes | 1.52 s | 2.04 s | 2.76 s |
| `-gen-intrinsic-enums` | `enums.inc`, 32,736 bytes | 8.15 s | 10.17 s | 17.35 s |
| `-gen-intrinsic-impl` | `impl.inc`, 3,203,861 bytes | 24.29 s | 35.75 s | 37.86 s |
| `bad.td` | `bad.td:2:17: error: Couldn't find class 'Nothing'`, exit code 1 | | | |

Each output is compared byte for byte with the host's. They are identical on both machines.

`clib` gained 34 checks of the new C library parts (104 now):
- `stat` of a file, a directory and a missing name; the same inode from two paths; `fstat` of standard output (a character device);
- `dup2` onto descriptor 20 and reading through it; `fcntl`;
- `mmap` of `welcome` showing its text, and `PROT_EXEC` refused;
- `mkdir`, `chdir`, `getcwd`, `truncate` shorter and longer (zeros);
- `opendir` listing what is there, `rmdir` of a full directory refused, then allowed once it is empty;
- a signal handler, a blocked signal kept pending until unblocked, `SIGCHLD` ignored by default, `kill` of another process refused;
- every pthread rule above; `sysconf`, `nanosleep` of 20 ms measured by the monotonic clock, and `rand()` after `srand(1)` giving the C standard's 16838.

`clib` runs in the normal boot, and `tcc64-test` and `aarch64-tcc-test` build it with TCC on TheTaaJ too, from the C library's sources. That now means 31 files, the five new C files among them, compiled byte for byte as on the host. `cxxlib` gained a group for one thread (62 checks): mutex, recursive mutex, `thread_local`, `call_once`, `wait_for` timing out after 50 ms, and `std::thread` throwing `resource_unavailable_try_again`. On the host, against glibc and libstdc++, the same file has 61 checks; the last one is TheTaaJ's.

On the host, `make libc-test` adds tests of `struct stat`'s layout (it must match the header's), of the inode hash, and of `strerror_r`.

Mutation experiments, 5:
- M1, `LLVMTableGenCommon` linked from its archive again: `llvm-tblgen did not finish all four of its jobs with exit code 0`. Caught.
- M2, a held mutex locked again without `EDEADLK`: `clib: FAILED: a mutex: held, and the deadlock reported`. Caught.
- M3, `__emutls_get_address` not copying the variable's first value: `cxxlib: FAILED: a thread_local variable`. Caught.
- M4, every file given inode 1: `clib: FAILED: stat of a directory` (a directory and a file had the same number). Caught.
- M5, the small size classes turned off (`SMALL_MAX` 0): the outputs are still right, but the intrinsics job took 87.90 s instead of about 9 s, and the next job did not finish within the test's time. Caught, by the clock only. It is a speed change, not a correctness change, so only a time limit can see it.

The testing note's steps were run as scripted boots on fresh scratch run disks on both machines, made by `make llvm-put` and `make llvm-put-aarch64`. Clippy is clean for `libc` and `user` on both CPUs. `make test64` passes.

Not tested:
- A person typing.
- `make llvm-put` on a run disk that is already nearly full. The program and its files take 5.5 MB of the 16 MiB disk.
- Most of Support. `llvm-tblgen` uses files, memory, command lines and error reporting; it does not spawn programs, use signals for crashes, or map files for writing.

## What is still missing

- **Clang itself**, which is the next steps: the code-generation libraries (E2), `clang -c` (E3), and lld (E4).
- **Bigger programs need a bigger kernel limit.** `llvm-tblgen` is 3.6 MB and programs may be 4 MiB (`PROGRAM_IMAGE_MAX`). Clang will be tens of MB.
- **TaajFS names are at most 27 bytes.** Some of AArch64's `.td` files have 28-byte names, so the AArch64 target's own TableGen jobs cannot run on TheTaaJ yet.
- **Reading files is slow**: about 1 KiB per request to the file service.
- **The X86 register tables need about 464 MB** when the host writes them, more than the 512 MB QEMU machine has.
- **Small blocks are never given back.** Once memory has been cut into 16-byte blocks, it stays in that class.
- **No threads, no `fork` or `exec`, no file locks, no `statvfs`.** LLVM works around each; Clang's driver will need some way to run programs.
- **LLVM is compiled with `-w`.**

## What I learned

- CMake does two things: it lists the sources, and it finds out about the system. The first can be read straight from its files. The second is a list of questions (44 answers in `llvm-config.txt`), and for a system I wrote myself I already know the answers.
- A static constructor that registers something only works if the linker keeps its file. Archives hand the linker files only on demand, so a file that nothing calls is dropped, even though running it was the point.
- "Works with one thread" is a real implementation, not a stub. A mutex that reports its own deadlock is more useful than one that hangs.
- A data structure can be correct and still the whole problem. The old heap gave every right answer; it was just O(n) per call. Nothing noticed until a program made millions of objects. M5 shows the same thing from the other side: only a clock can catch a speed regression.
- Comparing output byte for byte with the host is a very strong test for a program like TableGen. Any difference anywhere in Support, libc++ or the C library that changed a single character would have shown.

## Testing note

**1. The PC: llvm-tblgen**

```
make llvm-put
make run64
```

The first `make llvm-put` builds LLVM's Support and TableGen for the PC, about 230 files, which takes a few minutes. It puts `llvm-tblgen`, the 25 `.td` files it reads (under `src/llvm`) and a broken `bad.td` on the run disk. Close QEMU before running it.

| Type | You should see |
|---|---|
| `with readonly llvm-tblgen --version` | `TheTaaJ LLVM version 18.1.3`, `  Optimized build.` |
| `time with files llvm-tblgen -gen-vt -I src/llvm/include src/llvm/include/llvm/CodeGen/ValueTypes.td -o vt.inc` | `took` about 2 s |
| `time with files llvm-tblgen -gen-intrinsic-enums -I src/llvm/include src/llvm/include/llvm/IR/Intrinsics.td -o enums.inc` | `took` about 9 s |
| `with files llvm-tblgen bad.td` | `bad.td:2:17: error: Couldn't find class 'Nothing'`, the line `def something : Nothing;` with a `^` under `Nothing`, then `llvm-tblgen exited with code 1` |
| `ls` | `vt.inc` 18018, `enums.inc` 32736, `llvm-tblgen` 3634824, `bad.td` 38, and `src/` |

**2. The C library's new parts**

`clib` reads `welcome` and `docs/`. Make them first if they are not there yet:

| Type | You should see |
|---|---|
| `mkdir docs` | nothing printed |
| `echo Hello from the host > welcome` | nothing printed |
| `with files clib` | `library: 104 checks, all passed`, then `library: atexit ran its handlers, newest first` |
| `cxxlib` | `c++: 62 checks, all passed`, then `c++: the objects outside main were destroyed` |

**3. AArch64**

Close any AArch64 QEMU first.

```
make llvm-put-aarch64
make run-aarch64
```

The same as parts 1 and 2, typed at the terminal. Its disk already has `welcome`. The `-gen-vt` job takes about 3.5 s, and the intrinsics job about 19 s. `llvm-tblgen` is 3171856 bytes.

**4. The tests by themselves**

```
make libc-test
make llvm64-test
make aarch64-llvm-test
```

| Look for | Meaning |
|---|---|
| `llvm-tblgen on TheTaaJ (x86_64) read 25 .td files and wrote, byte for byte as the host's: vt.inc (18018 bytes) enums.inc (32736 bytes) impl.inc (3203861 bytes); took …` | all three jobs, and their outputs equal to the host's (`aarch64` for the other) |
| `llvm64-test passed`, `aarch64-llvm-test passed` | each machine |

Automated: make test64
