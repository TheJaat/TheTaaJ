# /dev

*Journal entry #NNN · J65 of Era 14: null, zero, full, random and urandom from a small service of their own, mounted at `/dev`; and `/dev/stdin`, `/dev/stdout`, `/dev/stderr` and `/dev/tty` from libc. Written on TheJat-VM/files, on the branch `j65-dev` in TheTaaJ and TheKernel.*

## Why

Programs and scripts name a few files that are not files at all:

- `> /dev/null` throws output away. It is in nearly every shell script, and in programs that need a descriptor that goes nowhere.
- `/dev/zero` gives zeros: to fill a file, or to map memory on old systems.
- `/dev/urandom` gives random bytes. Before `getrandom` (Linux 3.17, 2014) it was the only way, and much ported code still opens it.
- `/dev/full` fails every write with `ENOSPC`, so that a program can be tested for what it does when the disk is full.
- `/dev/stdout`, `/dev/stderr`, `/dev/tty` let a program, or a script given a file name, write to its own output or terminal.

TheTaaJ had none of them, so all of this failed with `ENOENT`. Entry 118's namespace and entry 126's `/tmp` showed the way: a service of its own, speaking the file protocol, mounted at a path.

## What it is

- **The `dev` service** (`user/src/bin/dev.rs`). `init` starts it with one request channel (grant 55) and registers its other end as `dev` (grant 56). It serves five devices:

  | Device | Reads | Writes |
  |---|---|---|
  | `null` | nothing: the end at once | taken, and forgotten |
  | `zero` | zeros, at any offset, forever | taken, and forgotten |
  | `full` | zeros | fail: no room (`ENOSPC`) |
  | `random` | random bytes, forever | taken, and forgotten |
  | `urandom` | the same: one generator, which never blocks (J2) | taken, and forgotten |

  Each one's size is 0, as on Unix. Nothing can be made in `/dev`, removed or renamed (`EROFS`), and a device is no directory (`ENOTDIR`).
- **The devices are a `Store`** (`fileserver/src/dev.rs`, `DevStore`), like the disk's and memory's, and tested on the host the same way. The service is the request loop `fs` already had, moved into `user/src/bin/common/serve.rs` so both programs include it, with this store in place of a disk.
- **Mounted at `/dev`** in every program given it: `taaj::file_service()` mounts `dev` beside `tmp`. The shell gives `dev` with `files`, whether the files are to change or only to read, and its own `Files` mount it too, so `echo hi > /dev/null` works at the prompt.
- **`/dev/stdin`, `/dev/stdout`, `/dev/stderr` and `/dev/tty` are libc's** (`standard_device` in `libc/src/descriptors.rs`). They are the program's own descriptors, not the service's, so only the program can open them:
  - the first three are a second descriptor for 0, 1 or 2;
  - `/dev/tty` is the keyboard when opened to read (`ENXIO` if the program's input is a file or another program), and the screen when opened to write.
- **A program in a box has no `/dev`.** `addroot_np` (entry 120) takes `dev` out of the child's registry, along with the other file services.

## How it works

```
init:   channel dev_in/dev_out; start "dev" with [dev_in]
        registry: "dev" = dev_out (DELEGABLE)
dev:    serve("dev", DevStore::new(taaj::random), dev_in, None)   (common/serve.rs)

program start (taaj::file_service):
        files = Files on "files", in its directory
        mount "tmp" at /tmp, "dev" at /dev, each if the program was given it

open("/dev/zero")   -> route: mount "/dev" -> "zero" on dev's channel -> DevStore::read: zeros
open("/dev/stdout") -> libc sees the name before asking anyone -> dup of descriptor 1
```

`DevStore` is a table of five names. A path is the root (`""`), one of the names, or nothing. `read` ignores the offset: every place in `/dev/zero` is a zero. `write` returns size 0, because the device holds nothing. `truncate` succeeds and changes nothing, because `> /dev/null` opens the file with `O_TRUNC`, and that must work.

The random bytes come from a function the service passes in (`taaj::random`, the kernel's generator). The crate stays free of system calls, and a host test passes its own: sevens, so the test can see where they went, and one that fails.

## Design decisions

- **A service, not libc pretending.** libc could fake `/dev/null` by itself. But then a program that doesn't use libc (a Rust program on `taaj`, or a C program passing the file to a child) would find nothing there. As a service it is a real directory: `ls /dev` lists it, `stat` works on it, a child is handed it like any other mount, and the shell's own redirection uses it with no special case.
- **But `/dev/stdout` and friends are libc's.** They mean "this program's own descriptor", which only the program knows. A service can't hand a program back its own standard output. On Linux they are links into `/proc/self/fd`, which the kernel resolves for the process asking. Here libc is that process's side, so it resolves them.
- **One channel, no read-only one.** `files` and `tmp` each have a read-only channel, so a program can be given look-but-don't-touch. Nothing in `/dev` can be changed, so a read-only `/dev` would protect nothing, and would make `> /dev/null` fail in read-only programs. Every program given files gets the one `dev` channel.
- **If the generator fails, the read fails.** Zeros in place of random bytes would be a key nobody knew was weak. `DevStore::read` on `urandom` gives `EIO` instead. The kernel's generator never fails for a program's own memory, so this shouldn't happen, but the error is there if it does.
- **A new program, embedded in the kernel like `fs`.** `/tmp` reused `fs` because `fs` without a disk already was a memory file system. Devices need a different store, so `dev` is its own small program. Both include the request loop from one file, so the protocol has one server-side implementation. The kernel image grew by the program's size, and `big64-test` (the kernel near its 3 MiB limit) still passes.
- **A box gets no `/dev`.** Nothing in `/dev` leads out of a box: `/dev/null` can't name a file. But "a box is what it was given" is easier to check than "a box is what it was given, plus whatever seemed harmless". A program in a box that needs `/dev/null` can be given it as a mount when there is a way to ask for one.

## Bugs met

None. The 24 checks passed on the first boot, on both CPUs. The device store had been tested on the host first, and `/dev` uses the mounts and node channels that entries 118 to 126 had already tested.

## How I checked it

- **On the host**: `cargo test` in `fileserver`, 3 new tests:
  - the listing, in order and from an offset;
  - each device's reads and writes, and a failing generator reading as `EIO`;
  - nothing made, removed or renamed, and the right error for each bad name.
- **The `files` lane's boot test**, `handles64-test` and `aarch64-handles-test`, still one boot per architecture, now runs `dev.c` (24 checks) after the other four programs:
  - `/dev` is a directory, in the root's listing, and lists all five;
  - `/dev/null` reads nothing and takes writes, opened with `O_CREAT | O_TRUNC` as `>` opens it, and through `fopen`;
  - `/dev/zero` reads zeros, even 1 MiB in;
  - `/dev/urandom` and `/dev/random` give different bytes each time;
  - `/dev/full`: `ENOSPC`, and zeros to read;
  - `EROFS` for a new file, a directory, a removal or a rename; `ENOTDIR` under a device;
  - `/dev/stdout` writes to standard output (the script finds the line); `/dev/stderr` and `/dev/stdin` open; `/dev/tty` opens and `isatty` says yes;
  - a child has `/dev`; a child in a box has none.
- Then the shell: `echo dev-hidden > /dev/null` prints nothing, and the next command runs.
- `boot64-test`: the shell's names, its grants and the root's listing (4 directories) with `dev`.
- `big64-test`: the bigger kernel still loads.
- `make test64`: see the testing note's last line.
- **Not tested**: `/dev/tty` to read. The test's input is the keyboard only when typed at, and a test that waits for a key would hang.

## What is still missing

- **`isatty` still decides by itself.** It asks libc's own table, not the terminal. J65 meant to have `isatty` ask the terminal, but there is no terminal service to ask yet: the screen and keyboard are the shell's channels. That waits for a terminal or pty service.
- **No other devices.** `/dev/fd/N`, the disks (`/dev/sda`), `/dev/ptmx` and pseudo-terminals, sound and input devices. Each needs its own service, which can then be mounted under `/dev` the same way.
- **`stat` says "regular file"** for a device. TheTaaJ's `Kind` has only files and directories. A program that checks `S_ISCHR` will be surprised.
- `/dev/stdin` and the rest are a second descriptor sharing the original's offset. On Linux, opening `/dev/stdin` on a file gets a new offset.

## What I learned

- **"Everything is a file" works because the file protocol is the interface.** `/dev/null` in Unix is a driver in the kernel that ignores writes. Here it is a 150-line `Store` and a 50-line program, and nothing else in the system knew it was coming. The namespace routes to it like any mount, and the shell's `>` uses the same open as for any file.
- **Some "files" belong to the program, not to any server.** `/dev/stdout` can't be served from outside, because only the program knows what its descriptor 1 is. That is why Linux needs `/proc/self`: the kernel makes up the answer for whoever asks. A microkernel has no such central "self" to ask, so the answer is in the program's own library.
- **Rust's function pointers keep a crate free of system calls.** `DevStore::new(random: fn(&mut [u8]) -> bool)` takes a plain function, the same idea as a C function pointer. The service passes one that calls the kernel, and the test passes one that fills sevens. Because `fn` (lowercase) is a pointer to a function with no captured state, `DevStore` needs no generic parameter and no `Box`. A closure that captured something would need `Box<dyn FnMut>` or a type parameter instead.

## Testing note

Start TheTaaJ:

```
make run64
```

| Type | You should see |
|---|---|
| `with programs devtest` | `dev: 24 checks, all passed` |
| `echo hi > /dev/null` | nothing printed |
| `ls /dev` | `full`, `null`, `random`, `urandom`, `zero` |
| `ls /` | `dev/` among the directories |

The same on AArch64 with `make run-aarch64`: `with programs devtest` gives the same line.

Automated: `cargo test` in `fileserver`, `make handles64-test aarch64-handles-test`, and `make test64`.
