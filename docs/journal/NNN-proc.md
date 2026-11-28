# /proc

*Journal entry #NNN · J66 of Era 14: the kernel's numbers as files, from a small service of its own mounted at `/proc`; and `/proc/self`, `/proc/self/exe` and `/proc/self/fd/N` from libc. Begun on TheJat-VM/files on the branch `j66-proc` in TheTaaJ and TheKernel (written, not yet compiled), and finished on TheJat-WSL while TheJat-VM is away. Written by day, with quick checks only.*

## Why

Programs read `/proc` to learn about themselves and the machine:

- `/proc/self/exe`: "where is my own file?" Compilers (clang, rustc) and anything that finds its libraries next to itself ask it.
- `/proc/self/fd/N`: one of the program's own descriptors, by name - what `/dev/stdout` is on Linux.
- `/proc/uptime`, `/proc/meminfo`, `/proc/cpuinfo`: how long the machine has run, how much memory it has, how many CPUs.
- `/proc/PID/status`: what `ps` and `top` show, for one process.

TheTaaJ answered none of them: `ENOENT`. The kernel already says all of it through the system handle (`system_info`, `process_list`, which `ps`, `mem`, `uptime` and K141's `top` use). J66 shows it as files.

## What it is

- **The `proc` service** (`user/src/bin/proc.rs`), built into the kernel image like `dev`. `init` starts it with its request channel (grant 60) and the system handle (grant 9), and registers the channel's other end as `proc` (grant 61).
- **Its files are a `Store`** (`fileserver/src/proc.rs`, `ProcStore`), tested on the host with made-up numbers. The service's request loop is the one `fs` and `dev` use (`common/serve.rs`).

  | File | Says |
  |---|---|
  | `uptime` | seconds since the clock started, and the CPU time spent busy (all CPUs added up), to the hundredth: `18.67 1.93` |
  | `meminfo` | `MemTotal`, `MemFree` (the frames, in kB), `HeapUsed`, `HeapTotal` (the kernel's own heap) |
  | `cpuinfo` | one `processor : N` line per CPU |
  | `stat` | `Processes`, `Threads`, `Channels`: how many there are |
  | `PID/status` | `Name`, `Pid`, `State` (running, or ended with its code), `CPU` (ns), `Memory` (kB), `Threads`, `Handles` |

  The text is made at each request, from what the kernel says then. A file's size is its text's length now, so `stat` and the listing agree with what a read will get.
- **Mounted at `/proc`** in every program given it: `taaj::file_service()` mounts `proc` beside `tmp` and `dev`. The shell gives `proc` wherever it gives `dev`.
- **`/proc/self` is libc's** (`own` in `libc/src/at.rs`). The service can't know who is asking, so libc turns `/proc/self/...` into `/proc/PID/...` before it asks. Two names are answered by libc alone:
  - `/proc/self/exe` is the program's own file, by the name it was started from (`__taaj_executable()`); `readlink` of it gives that name;
  - `/proc/self/fd/N` is a second descriptor for N, as `/dev/stdout` is for 1 (`standard_device`).
- **A program in a box has no `/proc`**: `addroot_np` takes `proc` out of the child's registry with the other file services.

## Design decisions

- **A service, as `/dev` is** (entry 127). A program without libc still finds `/proc` there, `ls /proc` lists it, and a child is handed it like any other mount.
- **Read-only at the door.** Nothing in `/proc` can be changed, so `ProcStore` says so (`Store::read_only`, new, false by default), and every channel to it is then read-only. An open to write is refused at once with `EROFS`, as on Linux, not at the first write.
- **No `self` in the service.** On Linux the kernel resolves `/proc/self` for whoever asks. Here the asker is a channel, not a process, so the program's own library resolves it. It is the same reason `/dev/stdout` is libc's.
- **The numbers the kernel already gives, nothing new.** No new system call: the service holds the system handle, which only looks, as the shell's `ps` holds it.

## Taken over

TheJat-VM had written this much before it went away: the store and its 3 host tests, the service, init's and the shell's changes, libc's `self`, `exe` and `fd`, and `proc.c` (27 checks). None of it had been compiled.

What finishing it took:

- **Main's numbers.** `j55-bin` (with the day's queue) merged in: `/proc`'s grants moved from 57 and 58 to 60 and 61, because the system log and tracing now hold 57 to 59. The shell's lists gained `proc` beside the `SYSTEM`, `DEBUG` and `TRACE` programs' entries. The root now has 7 directories (`proc` and `var`).
- **It compiled once three things were fixed:**
  - `ToString` was not imported in a `no_std` crate;
  - `ProcessEntry` had grown K141's `memory`, `threads` and `handles`. `status` now shows them too.
  - `proc.c` used `fscanf`, which TheTaaJ's libc doesn't have. It now uses `fgets` and `sscanf`.
- **Two of the 27 checks failed on the first boot:**
  - **`opendir("/proc/self")` found nothing.** `opendir` asks the namespace directly, not through `place()` where libc turns `self` into the PID. It now does that too.
  - **`open("/proc/uptime", O_WRONLY)` succeeded.** The service's channel allowed changes. Only the write would have failed. Hence `Store::read_only`.
- **The shell's `cat /proc/uptime` was "unknown command".** Since J55 `cat` is on the disk, in `/bin`, and the files lane's test disk had none. `handles-test.sh` now puts `/bin/cat` there.

## How I checked it

- **On the host:** `cargo test` in `fileserver`. There are 54 tests; `proc`'s 3 cover:
  - the listing, in name order;
  - each file's text, with a read in the middle and one past the end, and a silent kernel read as an error;
  - every change refused, each with the right error.
- **`handles64-test` and `aarch64-handles-test`** run `proc.c` after the other five programs. Its 27 checks:
  - `/proc` is a directory in the root's listing, and lists its four files;
  - `uptime` grows between two reads; `meminfo`, `cpuinfo` and `stat` parse, also through stdio;
  - the program's own `PID/status`, also as `/proc/self/status`; `/proc/self` lists `status`;
  - `/proc/self/exe` is the program's file (its size, an ELF header, `readlink`); `/proc/self/fd/1` writes to standard output;
  - `ENOENT`, `EROFS` and `ENOTDIR` where they belong;
  - a child sees itself and its parent; a child in a box has no `/proc`.
  - Then the shell's `cat /proc/uptime` must print two numbers. All passed, on both architectures.
- **Also passed:**
  - `boot64-test` (the names, the grants, and 7 root directories with `proc`) and `aarch64-keys-test`;
  - `power64-test` (`proc` stops at shutdown), `top64-test`, `log64-test`;
  - `big64-test`: the kernel with `proc` in it still loads;
  - `unsafe-check`.
- The full `make test64` is for the night.

## What is still missing

- **The LLVM patch is not removed.** LLVM's `getMainExecutable` is patched to call `__taaj_executable()` (entry 75) where Linux's reads `/proc/self/exe`. Now it could read the file. Removing the patch means rebuilding LLVM. That belongs with the paused `llvm-23` step, which is where LLVM changes now.
- **`/proc/self` is only libc's.** A Rust program on `taaj` (`cat`, for one) has `/proc/PID`, not `self`: `taaj`'s `Files` would need the same turn libc makes.
- **`/proc/PID/fd`, `cmdline`, `maps`, `environ`** for other processes. The system handle's calls don't give another process's descriptors or memory map. Each would need a call that only looks.
- **Only the first 32 processes** are listed: as many as one `process_list` call gives.
- **`ps` and `top` still ask the kernel themselves.** They could read `/proc`, but nothing would be gained yet.

## Testing note

Start TheTaaJ:

```
make run64
```

| Type | You should see |
|---|---|
| `cat /proc/uptime` | two numbers, like `18.67 1.93` |
| `cat /proc/meminfo` | `MemTotal`, `MemFree`, `HeapUsed`, `HeapTotal` |
| `ls /proc` | the processes' numbers as directories, then `cpuinfo meminfo stat uptime` |
| `ps`, then `cat /proc/PID/status` with a number it shows | that process's `Name`, `Pid`, `State` and numbers |

The same on AArch64 with `make run-aarch64`.

Automated: `cargo test` in `fileserver`, `make handles64-test aarch64-handles-test`, and `make test64`.
