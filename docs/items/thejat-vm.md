# TheJat-VM: the road to rustc on TheTaaJ (Era 9), and what it needs

*Part of the work list. The rules, who is doing what now, and what waits for the user are in [`docs/who-does-what.md`](../who-does-what.md); the lanes in [`docs/lanes.md`](../lanes.md).*


The order matters here: each item is needed by the ones after it.

### Before R3: what Rust's std will ask libc and the kernel for

| # | Item | Size | Notes |
|---|---|---|---|
| J1 | **Real thread-local storage (ELF TLS)**: `PT_TLS` loaded per thread, the FS base / `TPIDR_EL0` pointing at a proper TLS block, `__tls_get_addr` for the general model | M | **Done (entry 099)** for static programs (local-exec); C++ still emulated until step E; no `__tls_get_addr` (no shared libraries) |
| J2 | **`getrandom`** in the kernel: RDRAND/RDSEED on x86, `RNDR` or virtio-rng on AArch64, mixed with timer jitter into a ChaCha20 generator; `getrandom()`, `getentropy()`, `/dev/urandom`-like call in libc | M | **Done (entry 099)**: ChaCha20, RDSEED/RDRAND/RNDR + timings; no virtio-rng, no `/dev/urandom` file |
| J3 | **`poll`** (and `select` on top of it) over pipes, files, sockets and the keyboard | M | **Done (entry 100)**: in libc, over the read ends' channels; sockets always "ready" until the network service says when data comes |
| J4 | **`sigaltstack`** and running a handler on it; SIGSEGV on a stack overflow reaches that handler | S | **Done (entry 099)** |
| J5 | **`dl_iterate_phdr`** (for the program itself only, no shared libraries) | S | **Done (entry 100)**, with `getauxval` |
| J6 | **`pthread_getattr_np`**, `pthread_attr_getstack`, `pthread_sigmask` | S | **Done (entry 100)**: glibc's 56-byte `pthread_attr_t`, `pthread_attr_setstack`, `pthread_getattr_np` (the main thread's too); `pthread_sigmask` was already there |
| J8 | **`realpath`, `getcwd`, `fchdir`, `opendir` on handles** | S | **Done (entry 100)**: `realpath` and `getcwd` were there; `open` with O_DIRECTORY, `fchdir`, `fdopendir`, `dirfd` added |
| J9 | **`posix_spawn` file actions for fd 3+** (E11 left over from Era 6) | M | **Done**: entry 092 (E11) for spawn; entry 100 checks it through exec too |
| J10 | **`waitid`, `WNOHANG` everywhere, SIGCHLD** | S | **Done (entry 099)**: PROCESS_NOTIFY; WNOHANG and WNOWAIT. MaK-WSL fixed an ordering bug in it (entry 105): the exit code is now published after the watchers' signals, so SIGCHLD's handler has run when `waitid` returns |
| J11 | **Wide characters**: `wchar_t` functions, `mbrtowc`, `wcrtomb` in UTF-8 | M | Also helps libc++'s missing parts |
| J12 | **`dlopen`/`dlsym` stubs answering "unsupported"** honestly | S | **Done (entry 100)**: `dlopen(NULL)` works, `dlsym` says no, `dlerror` says why |
| J13 | **zlib** ported (for rustc's metadata compression) | S | **Done (entry 100)**: zlib 1.3.2 in `ports/zlib`, `make zlib64`/`zlib-aarch64` |
| J14 | **LLVM 23** for TheTaaJ (our port is 18; rustc 1.100 needs 23) | L | Redo E1-E4's port on the new version, byte-for-byte checks again |

### Era 9 proper

| # | Item | Size | Notes |
|---|---|---|---|
| J15 | **R3 Rust's std**: `x86_64-unknown-taaj` and `aarch64-unknown-taaj`, std over our libc, `panic = "abort"` first | L | Done when files, env, `Command`, threads and time work on both arches |
| J16 | **R3b unwinding**, if rustc needs it (libunwind for TheTaaJ, `.eh_frame_hdr`) | M | |
| J21 | **R4 rustc runs on TheTaaJ**, cross-built with `x.py`; `taajfs.rlib` byte for byte as the host's | L | |
| J22 | **R5 a Makefile driving rustc without cargo**, checked against cargo on the host | L | |
| J23 | **R5 nasm ported**; the BIOS loader built with clang (`--target=i686-elf`) | M | The bootloader change itself is MaK-WSL's (rule 5); TheJat-VM ports nasm |
| J24 | **R6 our own ISO 9660 / El Torito writer** (a Rust program) | M | |
| J25 | **R6 the milestone**: `make` on TheTaaJ builds everything, the image boots and passes test64, and builds itself again to the same bytes | L | |

### Memory, also needed for big builds

| # | Item | Size | Notes |
|---|---|---|---|
| J29 | **Programs loaded page by page from their file** (not whole, not twice in memory) | M | A 182 MB rustc. Needs J58: sharing the program object's frames as code is only safe once nobody can write them (entry 103) |
| J58 | **Sealed memory objects**: an object made read-only for good - no writable mapping or handle left - checked by the kernel before it shares the frames | M | For J29; maybe for real shared file mappings later |

### C and C++ library, left over from Era 6

| # | Item | Size | Notes |
|---|---|---|---|
| J32 | `scanf`/`fscanf` from streams | S | Only `sscanf` today |
| J33 | `%a` in printf; long double done properly (`strtold`, `%Lf`) | M | |
| J34 | `erf`, `erfc`, `tgamma`, `lgamma` and the long double math | S | Declared, not defined |
| J36 | libc descriptors follow the file, not the name (rename/remove of an open file) | M | **Done (entry 115)**, by J60 |
| J37 | `getenv`/`setenv` with a real environment passed to children | S | **Done** before this list (`environ`, posix_spawn's envp; checked through exec in entry 100) |
| J38 | `execvp`, `PATH` search; a `/bin` directory | S | **Done (entry 100)**: the exec family as spawn-and-wait with the exact exit code; PATH, or `/` then `/bin`. The `/bin` directory itself waits for step D |
| J39 | `argv[0]` is the program's real name (not "program") | S | **Done**: argv[0] is the parent's (checked in entry 100) |
| J40 | libc++ `<filesystem>`, `random_device` (after J2), wide streams | M | |
| J41 | Double `free` and overrun detection in `malloc` (a debug mode) | S | |
| J42 | Host tests for `malloc`'s family | S | |

### Developer tools on TheTaaJ

| # | Item | Size | Notes |
|---|---|---|---|
| J43 | Shell: variables, `if`, `for`, `&&`, `||`, `;`, wildcards | M | Build scripts need them. `sh` has them; Era 20's J82 brings them to the shell at the keyboard |
| J44 | Shell: `2>`, `2>&1`, here-documents, `exec` for the last command | S | `sh` has them; Era 20's J82 |
| J45 | Rebuild only what changed in `/src/build` (make with real dependencies) | S | |
| J46 | Install the built libc and programs over `/lib` and the root | S | |
| J47 | `cp -r`, `rm -r`, `mv`/`cp` with many names, `find`, `grep`, `diff`, `wc`, `sort`, `head`, `tail` | M | |
| J48 | A pager (`less`-like) and per-command help | S | `help` is at the screen's limit |
| J49 | Editor: undo, selection, copy and paste, search, go to line, line wrap | M | |
| J50 | `git`-like tool, or at least `patch` | L | Later |

### Test suite health

| # | Item | Size | Notes |
|---|---|---|---|
| J51 | Test programs out of the kernel image (onto the test disk) | M | The kernel image has ~680 KiB left under 3 MiB |
| J52 | A test that fails when rule 1 or rule 3 of `threads.rs` is broken | S | **Done (entry 119)**: rule 1 checked at every switch (the lock still held where it is released), rule 3 by the threads self-test (a waiter woken before it waits is still in its queue); each broken on purpose panics. Found and fixed with it: a panic on a machine with more CPUs than the kernel uses was a triple fault (the stopping NMI reached the CPUs left asleep); `fault-test.sh`'s new `page@20` checks it |
| J53 | A test for the ticks mutation from entry 097 (late timer interrupts) | S | **Done (entry 128)**: the late-interrupts self-test (35 ms with interrupts off); entry 097's mutation now caught on x86 (on AArch64 it changes nothing) |
| J54 | A fresh `git clone --recurse-submodules` builds and passes (in a clean container) | S | Never tried |
| J55 | Programs moved into `/bin` on the run disks (libc's exec and the shell both look there, entries 100 and 101) | S | The disk recipes and every test put programs in the root and look for them there |
| J56 | zlib (and later other libraries) installed on TheTaaJ's disk with its header, for programs compiled there | S | With step E, when there is a place for libraries on the disk |
| J57 | exec without the go-between: signals sent to it passed on to the program it started, and a keyboard handed on | M | Entry 100's exec is spawn-and-wait |
