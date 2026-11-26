# Std's ground, part 2: poll and select, dl_iterate_phdr, pthread attributes, exec with PATH, directory descriptors, zlib

*Journal entry #100 · The-Jat's step C (`docs/who-does-what.md`), on the branch `std-ground-2`. Step B (entry 099) gave libc the four things most other work waited for. This step is the rest of what Rust's standard library and rustc ask a Unix C library for, so that R3, porting std, is only Rust's side of the work:
- **`poll` and `select`**: std's process code waits on a child's pipes with poll; so do build tools, servers and terminals;
- **`dl_iterate_phdr` and `getauxval`**: how an unwinder (Rust's backtraces, C++'s exceptions) finds the program's headers, and where std asks for the page size and random bytes;
- **`dlopen`/`dlsym`**, answering honestly that there is nothing to load;
- **the pthread attributes**, with `pthread_getattr_np`: std reads where each thread's stack is, to tell a stack overflow from any other fault;
- **the exec family** with a `PATH` search: `std::process::Command` uses `execvp` in a child, and so do shells;
- **directory descriptors**: `fchdir`, `fdopendir`, `dirfd`;
- **zlib**: rustc compresses metadata with it, and so does much else.*

## Why

std's Unix code is written against glibc and musl. Where TheTaaJ's libc lacked a call, std would fail to link, or the port would need a special case for every gap. Each piece here was missing or a stub:
- **No way to wait for several descriptors.** A program reading two pipes, a child's output and its errors, had to pick one and block on it. If the child filled the other pipe first, both waited for ever.
- **No way to find the program's own headers.** The kernel loaded them and, since step B, can copy them out (PROGRAM_HEADERS). But nothing offered them in the standard shape an unwinder looks for.
- **`pthread_attr_t` was one `int`**: a detached bit and the stack size in KiB. There was no place for a stack the program provides, no guard size, and no way to ask a running thread where its stack is.
- **`execvp`, `execv` and `execve` failed with ENOSYS.** TheTaaJ has no call that replaces a running program, so a program that execs after setting up its descriptors, like a shell or a wrapper, simply didn't work.
- **A command was always a file in the root.** `PATH` was ignored, and there was no `/bin` to look in.
- **A directory could not be opened as a descriptor**, so `fchdir` and `fdopendir` had nothing to work on.
- **No zlib.** rustc's build needs it.

The step's list also had items that were already there: `stat` (types, sizes, a modification time), `realpath` and `getcwd`, the environment passed to children (`environ`, from entry 081's work), `argv[0]` as the parent gave it, and descriptors 3 and up for a child (entry 092, Era 6's E11). They are now checked here too, through exec as well as spawn.

## How it fits together

```
 a program                       libc                                        kernel / services
 ──────────────────────────      ────────────────────────────────────────    ─────────────────────
 poll(fds, n, timeout)           poll.rs   ask each descriptor:              OBJECT_WAIT(_UNTIL):
 select(...)                       pipe read end, stdin -> take in any         woken by a message
                                   message waiting (pipes.rs, taaj::Input)     or a closed peer
                                   write end -> peer closed? POLLERR
                                   file, socket, output -> ready
                                 none ready: wait on the read ends'
                                 channels until the deadline, look again

 dl_iterate_phdr(cb, data)       dynamic.rs  headers copied once            PROGRAM_HEADERS 40
 getauxval(AT_PHDR, ...)           (load bias 0, dlpi_tls_data from          (step B)
 dlopen / dlsym / dlerror          threads.rs), AT_RANDOM from getrandom

 pthread_getattr_np(t, &a)       threads.rs  each record keeps where its     abi::stack::TOP and
 pthread_attr_setstack(...)        stack is; the main thread's worked out    the size rule, shared
                                   from abi::stack and PT_GNU_STACK          with elf.rs

 execvp("prog", argv)            spawn.rs  start prog with everything exec   init starts it; this
                                   would leave it, wait, exit with its        program waits and
                                   exact code; a name looked up in PATH       ends with its code

 open(dir, O_DIRECTORY)          descriptors.rs, status.rs                  the file service
 fchdir, fdopendir, dirfd

 #include <zlib.h>               ports/zlib -> build/c/libz.a (x86_64 and AArch64)
```

### poll and select

The kernel knows channels, not descriptors. So poll does in libc what Linux does in the kernel. It asks each descriptor whether it is ready, and if none is, it waits on the channels behind the ones that might become ready:

| Descriptor | Ready when |
|---|---|
| a pipe's read end, standard input | bytes are held from an earlier message, or a message is waiting (it is taken in now): POLLIN. The other end closed and everything read: POLLHUP |
| a pipe's write end | always POLLOUT, until the reader has closed: POLLERR (a write would give EPIPE). Nothing is ever sent back up a pipe, so a write end whose channel is "ready" has a closed peer |
| standard output and error, a file | always: a file's read and write go to the file service and come back |
| a socket | always (see What is still missing) |

Taking a waiting message in is what makes the answer exact. A message that wakes the wait but carries no bytes (an empty one, or one that isn't part of the stream) is read and dropped, and poll looks again. A wake is only a hint.

Two threads can share a pipe. When another thread is in `read` on it, that thread holds the end's lock while it waits. poll uses `try_lock`: if the lock is taken, it doesn't block, but it still has the channel to wait on, and it looks again a tick later.

**A signal ends the wait with EINTR.** POSIX says poll and select are never restarted, whatever SA_RESTART says. Every other wait in the `taaj` crate restarts after a handler runs, so there is a new function for this, `object_wait_or_signal`.

**The timeout is measured on the CPU's counter, not in ticks.** nanosleep already did this (entry 097). See Bugs for why poll needs it too.

`select` builds a pollfd list from its sets and calls the same wait. A descriptor in a set that isn't open gives EBADF (poll would say POLLNVAL for it instead). `except` is always emptied: there is no out-of-band data. `fd_set` is Linux's 1024 bits. `sys/time.h` includes `sys/select.h`, as on Linux.

### dl_iterate_phdr, getauxval, dlfcn

Every program here is one static file: no dynamic linker, no shared objects. So `dl_iterate_phdr` calls back exactly once, for the program, with:
- **its program headers**: a copy, made at the first call from PROGRAM_HEADERS and kept for good;
- **load bias 0**: programs are linked where they run;
- **`dlpi_tls_modid` 1 and `dlpi_tls_data`**, this thread's TLS block, when the program has a PT_TLS.

Finding the block needed the TLS layout of entry 099 in a second place. That layout is now one function, `area_layout`, and both the code that builds a thread's area and `tls_block` call it.

**`getauxval`** answers what Linux's auxiliary vector would. The kernel passes none, so libc makes the answers up:
- AT_PHDR, AT_PHNUM, AT_PHENT: the copy above;
- AT_PAGESZ: 4096;
- AT_CLKTCK: 100;
- AT_RANDOM: 16 bytes from getrandom, made once;
- 0 for the ids, AT_SECURE and AT_HWCAP;
- anything else: 0 with ENOENT.

**`dlopen(NULL)`** gives a handle for the program. **`dlsym` always gives NULL**: the symbol table isn't in memory. std looks some functions up "if they are there" with dlsym, and does without them when they aren't, which is the honest answer here. `dlerror` says why, once.

### pthread attributes

`pthread_attr_t` is now glibc's 56 bytes: detached, the stack size, the stack's address if the program gives one, and the guard size. A Rust `const` assertion checks the size against the header's. Each thread's record keeps where its stack is, whoever made it.
- **`pthread_attr_setstack`**: the thread runs on the program's own memory, which the library never unmaps.
- **`pthread_getattr_np(thread)`**: detached or not, the stack, the guard size. For the main thread the stack is the program's, so libc works it out. It ends at `abi::stack::TOP` and is as big as the program's PT_GNU_STACK header asked for.

For the second part, the kernel's constants moved into `abi::stack`: TOP, MIN, DEFAULT, MAX and `size(wanted)`, the rule elf.rs used inline. The kernel and libc now read the same numbers. That way they can't drift apart.

### exec

TheTaaJ has no call that replaces a running program. Nothing a program holds, its handles or its memory, could be carried across one. So exec starts the new program as a process of its own, with everything exec would leave it:
- the arguments and the environment;
- the current directory;
- every descriptor not marked close-on-exec, 3 and up included;
- standard input.

Then it waits for that process and **ends with exactly its exit code**. A signal's exit code is passed on as it is too (abi::signal::exit_code). Whoever waits for this program sees what Unix would have shown: exit 7 is exit 7, killed by SIGTERM is killed by SIGTERM.

```
 shell ─ spawn ─► ground2 exec-to raise 15 ─ execvp ─► ground2 raise 15
                     (waits)                                raise(SIGTERM): ended by signal 15
                     exit(-1115)  ◄──────────────────────── its exit code
 waitpid: WIFSIGNALED, WTERMSIG = 15
```

The kernel's log shows the go-between as `exited with code -1115`, the code that means "signal 15". The waiting parent decodes it the same way either way.

The list forms, `execl`, `execlp` and `execle`, are C (unix.c), because they take `...`. They gather the arguments into an array for the `v` forms.

**PATH.** A name without '/' (posix_spawnp, execvp, execlp, execvpe) is looked for in each directory of `PATH` in turn. An empty entry means the current directory. The first directory that has a file of that name wins. With no PATH, the search is `/` and then `/bin`: commands have always been in the root here, and `/bin` is where a Unix program expects them.

### Directory descriptors

`open("gdir", O_RDONLY | O_DIRECTORY)` now opens a directory: a descriptor holding its path, as a file's does, whose `read` gives EISDIR. Without O_DIRECTORY, opening a directory still fails with EISDIR, as `cfile` and `cstdio` have always checked. A file opened with O_DIRECTORY gives ENOTDIR. On top of that:
- **`fchdir(fd)`** is chdir to the descriptor's path;
- **`fdopendir(fd)`** is opendir on it, and the DIR then owns the descriptor;
- **`dirfd(dir)`** returns that descriptor, or opens one when the DIR came from opendir. closedir closes it either way.

### zlib

zlib 1.3.2 is in `ports/zlib`: the fifteen C files and their headers, unchanged, with its licence. I took them from rustc's own vendored copy (`libz-sys`), which is the zlib rustc builds with. `make zlib64` and `make zlib-aarch64` compile it with the C programs' flags (`-Wall -Wextra -Werror` included, with no warnings) into `build/c/libz.a`.

zlib's configure script would normally write `Z_HAVE_UNISTD_H` into `zconf.h`. Here the Makefile passes it as `ZLIB_CFLAGS`. A program that includes `zlib.h` must be compiled with the same flags, so that both sides agree on what `zconf.h` declares.

## Design decisions

- **poll in libc, not in the kernel.** The kernel has no idea what a descriptor is, and shouldn't. Every kind of descriptor is a libc object over channels (or the file service), so libc is the only place that can say whether a read would wait. The kernel's part, waiting on several channels until a deadline, was already there.
- **poll takes waiting messages in, rather than peeking.** There is no "peek" on a channel. Taking a message into the end's held bytes is what `read` would do next anyway, and it makes POLLIN exact: no false "ready" for an empty message.
- **exec as spawn-and-wait.** The alternative was a kernel call that replaces the image in place. That would mean keeping handles across it, resetting threads and memory, and redoing the loader in the kernel, all to save one process. The go-between costs a process that sleeps, and it gives every observable result exec promises except the process ID.
- **The exit code passed on raw.** Re-raising the signal in the go-between would also work, but it needs the signal's default action restored first, and it races with handlers. Exiting with the very code the child ended with gives the same answer to the parent, in one step.
- **O_DIRECTORY required to open a directory.** POSIX allows `open(dir, O_RDONLY)`, but two existing tests check that TheTaaJ refuses it. Programs that want a directory descriptor (std's Linux code, `find`, `rm -r`) ask with O_DIRECTORY anyway.
- **dlsym says no, rather than searching.** Keeping the symbol table in memory would cost every program its size for a call almost nothing makes. A static program that wants a function by name can link it.
- **New items for what is left** (J55-J57, K49) go in `docs/who-does-what.md`, the one list, rather than staying only in this entry.
- **zlib from rustc's vendor directory, not a download.** It is the exact zlib rustc will be built with, its checksums are recorded by cargo, and nothing had to be fetched.

## Bugs

Three, all found by the new test:
- **exec failed with EPERM from a program at the keyboard.** exec hands down standard input, as Unix does. But a keyboard can't be handed on: the shell gives typed lines to whoever asked, and the handle has no DUPLICATE right. All the checks that exec from a child passed, because the children have no standard input (EBADF, which exec already allowed for). Only the `execvp` of a missing program, called by `ground2` itself, whose input is the keyboard, came back with errno 1 instead of ENOENT. Now exec also starts the program without input when its input can't be handed on (EBADF or EPERM).
- **`execle` read the wrong environment on AArch64.** The list helpers took the `va_list` by value. On x86_64 `va_list` is an array, so passing it passes a pointer: the helper's reads moved the caller's list on, and `execle` found the environment right after the null. On AArch64 `va_list` is a struct and is copied, so `execle` read its first argument again (`"env-answer"`) as the environment pointer, and the child got no ANSWER. The helpers now take `va_list *`. This is the classic portability trap with `va_list`; x86_64 alone would never have shown it.
- **select's 30 ms timeout sometimes came back early on AArch64**, about one run in three. poll worked out its deadline as "ticks now + 3". Under QEMU's emulation the timer ticks can come late and then in a burst, so three ticks could pass in well under 30 ms of real time. The deadline is now a time on the CPU's counter, and the wake-up tick is the first one at or after it, as nanosleep reckons. Waking early only means checking the time again. After the change, AArch64 passed three runs in a row.

One in an older test, found by `make test64`:
- **`clib` checked that `execvp` fails with ENOSYS** (entry 069, the TCC port, wrote it when that was all execvp did). `tcc64-test` and `libmake64-test` compile clib on TheTaaJ and failed with `clib: FAILED: execvp: ENOSYS`. The check now asks for a program that isn't there and expects ENOENT, so clib goes on running either way.

And two in the tools:
- **`tools/testing-notes.py` skipped this entry.** It collected `docs/journal/0[0-9][0-9]-*.md`, which stops at 099, so entry 100's note was silently left out of `docs/testing-notes.md`. Found by looking for it there after running the script. It now reads `[0-9][0-9][0-9]-*.md`.
- **The test script on AArch64:** the keys are typed at the serial port by `serial-type.py`, which knows `~` and `~~` but not `~~~`. With `~~~` it typed nothing at all, and the boot ended at the prompt. The script uses `~~` (up to 90 seconds).

## How I checked it

**Host tests:**
- the kernel's 217 unit tests, unchanged: elf.rs's stack-size tests now go through `abi::stack::size`;
- a new abi test, `the_stack_a_program_gets`: the default, MIN for a small request, rounding up to whole pages, MAX accepted and MAX + 1 refused.

**The new boot tests, `make ground2-64-test` and `make aarch64-ground2-test`.** One boot each. `with programs ground2` runs **87 checks**:
- **poll:**
  - an empty pipe isn't readable; its write end is writable;
  - bytes make it POLLIN, and it stays POLLIN while part is still held;
  - a 50 ms timeout waits about 50 ms;
  - **a poll with no timeout wakes when another thread writes**, and not before;
  - of four entries only the ready one and the one not open (POLLNVAL) are counted, and a negative descriptor is skipped;
  - **a SIGALRM handler ends poll with EINTR**;
  - after the writer closes, a byte still there is POLLIN, then POLLHUP;
  - a write end whose reader has gone says POLLERR;
  - a file says POLLIN and POLLOUT;
- **select:** readable and writable at once; a 30 ms timeout empties the set; EBADF for a descriptor not open;
- **across processes:** a child started with the pipe's write end at 3 and up writes after 200 ms; poll wakes, the bytes are there, and the child's end gives POLLHUP;
- **dl_iterate_phdr:**
  - one call, its answer returned;
  - the PT_LOAD this code is in;
  - a PT_TLS, module 1;
  - `dlpi_tls_data` holds this thread's thread-local, and in another thread that thread's;
- **getauxval:** page size; AT_PHDR and AT_PHNUM the same headers; AT_PHENT 56; AT_RANDOM 16 bytes, the same each time; ENOENT for an unknown type;
- **dlfcn:** dlopen(NULL); dlsym NULL with a dlerror, said once; dlopen of a library NULL; dlclose 0;
- **pthread attributes:**
  - the size is 56; the defaults (8 MiB, a page of guard); EINVAL below the minimum;
  - **the main thread's stack is 8 MiB and holds a local of main**;
  - a 256 KiB thread sees its own stack with its local in it, its guard size, and that it's joinable;
  - **a thread on a stack the program gave runs on it**;
  - a detached thread's attributes say so; ESRCH for a thread that isn't one;
- **exec:**
  - execvp passes on exit code 7, and **death by SIGTERM**;
  - execle passes the environment, and only it;
  - execvp and execl of nothing give ENOENT, and the program goes on;
  - **a pipe at 3 and up reaches the program exec started**; one marked FD_CLOEXEC before exec doesn't;
  - `PATH=/nowhere:/` finds the program in the second directory; `PATH=/nowhere` gives ENOENT; no PATH means the root;
  - **`argv[0]` is the name the parent gave**;
- **files:**
  - stat of the program; getcwd; realpath (ENOENT for nothing, `./ground2` to `/ground2`);
  - open of a directory without O_DIRECTORY gives EISDIR as before, a file with it ENOTDIR;
  - a directory with it opens, and reading it gives EISDIR;
  - fchdir into it; fdopendir lists a file made in it; dirfd is the descriptor; closedir closed it;
- **zlib:**
  - version 1.3.2;
  - **crc32 of "123456789" is CBF43926 and adler32 of "Wikipedia" is 11E60398**, the published check values;
  - 200 KB of text compresses to under a twentieth and comes back byte for byte;
  - a damaged stream is refused;
  - **gzwrite makes a file starting 1f 8b**, and gzread gives it back, then the end.

The script also requires that the kernel killed nothing and that no service stopped answering.

**Mutation experiments**, each undone, each run with `make ground2-64-test`:

| What I broke | What happened |
|---|---|
| poll ignores bytes already held by a read end | **caught**, as a hang: the poll with no timeout then waits for a message that has already been taken in, and the script says "the steps did not all run". (Without a timeout of its own, a hang is the only way this check can fail) |
| poll restarts after a signal instead of EINTR | **caught**, as a hang at the EINTR check: the alarm's handler runs and poll goes back to waiting for ever |
| no POLLERR for a write end whose reader closed | **caught**: "the reader gone: the write end says POLLERR" |
| `dlpi_tls_data` always null | **caught**: both TLS checks |
| a thread's record forgets where its stack is | **caught**: the 256 KiB thread and the given-stack thread both see the main thread's stack instead |
| exec passes on only the low byte of the exit code | **caught**: "and its signal: killed by SIGTERM" (exit 7 still passed, as it should) |
| the PATH search ignores PATH | **caught**: "PATH=/nowhere : posix_spawnp says ENOENT" |
| `pthread_attr_setstack` ignored by pthread_create | **caught**: "the thread ran on the given stack" |

**The testing note**, typed in a scripted boot on a scratch run disk (`make run-disk RUN_DISK=build/t100.img`, booted with `with programs ground2` typed): `ground2: 87 checks, all passed`, and in the log the children exactly as the note lists them. (The AArch64 run disk isn't built here: `make aarch64-ground2-test` boots the same program on AArch64.)

**`make test64`**: the first run failed only on clib's old ENOSYS check (above); after that fix it passed: `test64: all passed`, 14 min 15 s, every part including `ground2-64-test`, `aarch64-ground2-test`, `tcc64-test` and `libmake64-test`. Its log has one `qemu-boot: the shell did not run all 237 commands` from boot64-test: the boot script's 10-second wait for the shell's last line ran out on this slow machine, and every check boot64-test then makes on the log passed.

## What is still missing

- **A socket is always "ready" to poll.** A read on it may still wait. The network service holds a connection's bytes and libc can't tell how many without asking. Making it exact needs the service to say when data arrives (a message on the socket's channel that poll can wait for). That belongs with MaK's network work (K49, added to the list).
- **Polling the keyboard asks it for a line.** Standard input from the keyboard only sends something when asked (abi::stream::WANT), so poll asks, as read would. A program that polls stdin and then doesn't read it leaves a typed line waiting for itself. Programs that poll stdin read it next, so in practice this matches Unix.
- **No `ppoll`, `pselect` or `epoll`.** std doesn't need them; tokio and friends would want epoll.
- **exec keeps the go-between running** (and its memory), and the process ID changes. Signals sent to the go-between aren't passed on to the new program. And a keyboard isn't handed on: a program exec'd from one runs without input. (J57.)
- **dlsym always says no**, `dladdr` knows nothing, and AT_HWCAP is 0 (std uses it only for its own `is_x86_feature_detected`, which falls back to cpuid).
- **The guard size is said back, not honoured**: every thread stack has exactly the one unmapped page below it.
- **No `/bin` on the disk yet.** The search looks there, but programs still live in the root. Moving them is a layout change for the disk tools and every test, best done with TaajFS v4 (step D, J55).
- **zlib is built for linking on the build machine**, not installed on TheTaaJ's disk with its header for programs compiled there. That comes when there is a place for libraries on the disk, with step E (J56).
- **`stat`'s times** are one modification time; access and change times wait for TaajFS to keep them (J17).

## What I learned

- **poll is a libc question on a microkernel.** On Linux a descriptor's readiness is the kernel's, because the kernel holds the pipe's buffer. Here the buffer is half in libc (the bytes already received) and half in the kernel (messages not yet received). Only the code holding both halves can answer, and the kernel's part reduces to "wake me when a message comes".
- **`va_list` is not a value you can pass around.** Whether it is an array, a pointer or a struct is each ABI's choice. Code that passes it by value and then keeps using it is undefined everywhere, and it only *happens* to work where it is an array. Pass `va_list *`, or `va_copy`.
- **Timeouts belong on a clock, not on a tick count.** "Three ticks" is a promise about interrupts, and under emulation interrupts come late and then all at once. A deadline is a time, and waking early means checking again.
- **exec's promises can be kept without exec.** What a parent can observe of a child that execs: its exit status, its output on the descriptors it kept, its environment. All of that survives a spawn-and-wait. Only the process ID gives it away.
- **A test's children see a different world from the test.** The children had no standard input; the parent had a keyboard. The bug was only in the parent's world, and it showed because one check ran there.

**Rust, as used here:**
- **`Mutex::try_lock` returning `Option<MutexGuard>`**, written as `self.lock.try_lock().then_some(MutexGuard { mutex: self })`. `bool::then_some` turns "did it lock" into "here is the guard".
- **`AtomicPtr` for a value made once**: build it, `Box::leak` it, `compare_exchange` it in. A thread that loses the race uses the winner's and leaks its own few hundred bytes, once. That is cheaper than a lock on every call.
- **`const _: () = assert!(size_of::<Attributes>() == 56)`**: a compile-time check that the Rust struct and the C header agree.
- **`c"..."` literals** give a `&CStr` with its zero byte, for `dlerror`'s messages and `getenv("PATH")` without building a string at run time.
- **`Option::is_some_and`** for "the deadline exists and has passed", and **`Option::map_or`** for "no deadline means the longest wait".
- **A shared rule as a `const fn` in the abi crate** (`abi::stack::size`), callable from the kernel's parser and from libc. One definition, and a unit test, for both.

## Testing note

**1. The PC**

```
make run-disk
make run64
```

Close QEMU before `make run-disk`. It puts `ground2` on the disk.

| Type | You should see |
|---|---|
| `with programs ground2` | after a few seconds `ground2: 87 checks, all passed`; in the terminal, its children `exited with code 7` (twice: the program exec started, then the one that exec'd), `ended by signal 15 (SIGTERM)` then `exited with code -1115` (the go-between passing the signal's code on), `exited with code 42` twice, `exited with code 3` twice, `exited with code 4`, and at the end `"ground2" exited with code 0` |

**2. AArch64**

```
make run-aarch64-disk
make run-aarch64
```

The same as part 1.

**3. The tests by themselves**

```
make ground2-64-test
make aarch64-ground2-test
make test64
```

| Look for | Meaning |
|---|---|
| `std's ground, part 2 (x86_64): 87 checks of poll, select, dl_iterate_phdr, getauxval, dlfcn, pthread attributes, exec with PATH, descriptors through exec, argv[0], realpath and zlib passed` | every part above; the same for aarch64 |
| `test64: all passed` | every part, the new two included |

Automated: make ground2-64-test, make aarch64-ground2-test, make test64
