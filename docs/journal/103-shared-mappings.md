# Memory for big programs, part 2: shared file mappings that are written back, pread and pwrite, and why programs are not loaded page by page yet

*Journal entry #103 · The-Jat's step G, part 2 (`docs/who-does-what.md`), on the branch `big-memory-2`. Part 1 (entry 102) made pages protectable and memory returnable. This part:
- **J28, `MAP_SHARED` file mappings that write back**: what a program writes into a shared, writable mapping of a file reaches the file;
- **`pread` and `pwrite`**, which the write-back is made of, and which Rust's std uses for `FileExt::read_at` and `write_at`.

J29, loading programs page by page, I looked at and did not do. Why is below.*

## Why

`mmap(..., PROT_WRITE, MAP_SHARED, fd, ...)` failed with ENODEV. Programs that update a file in place through a mapping, rather than by writing it, couldn't run at all. Linkers do this (lld writes its output through a mapping when it can), and so do databases and some build tools. There was also no `pread` or `pwrite`, so a program that reads a file at several places from several threads had to `lseek` and hope no other thread moved the offset in between.

## How it fits together

```
 mmap(file, PROT_WRITE, MAP_SHARED)        the mapping, in libc (mmap.c)
   reserve, read the file in (as before)   { start, length,
   dup(fd) -> a descriptor of its own        fd = the dup, offset in the file,
                                             file_bytes = what came from the file }
   first one: atexit(write_all_back)

 msync(p, n)            ─┐
 munmap(p, n), all/part ─┼─► write_back: pwrite(fd, bytes, offset + (p - start)),
 exit (atexit)          ─┘     only within file_bytes: a mapping never makes a file longer

 pread(fd, buffer, n, at)    files.read_from(path, at)   - the descriptor's place untouched
 pwrite(fd, buffer, n, at)   files.write_from(path, at)
```

The file service is a separate program, and the kernel never sees a file, so the mapping stays what it was: a copy of the file's bytes in reserved memory. What changes is that a shared, writable mapping remembers where it came from and writes itself back at three moments:
- **`msync`**: the range asked for, or what of it is mapped;
- **`munmap`**: the part being unmapped, before its memory is discarded (entry 102), or all of it;
- **the program's exit**: every shared mapping still there, from an `atexit` handler registered with the first one (`pthread_once`).

Each mapping keeps a descriptor of its own, a `dup`, so the program may close its descriptor right after `mmap`, as Unix programs often do, and the write-back still has somewhere to go.

**`pread` and `pwrite`** do what `read` and `write` do on a file, at a given offset, through the same file-service calls (`read_from`, `write_from`), without touching the descriptor's offset. On anything but a file they fail with ESPIPE, and with EINVAL for a negative offset. `pwrite` writes where it is told even with O_APPEND, as POSIX says; Linux appends instead, a known Linux quirk.

## Design decisions

- **Copy-and-write-back, not real sharing.** Real sharing would mean the file's pages living in memory the kernel maps into every program that maps the file, with the file service as their pager. That needs the kernel to page from a user-space service: a pager protocol, page faults that wait for a message. It's the same machinery as J29. Copy-and-write-back gives each program what it relies on (its writes reach the file) and is honest about the rest. sys/mman.h says that a `read` of the file, or another program's mapping, sees a change only once it is written back.
- **The whole range is written back, not only the pages that changed.** Without dirty bits from the kernel, libc can't tell which pages were written. The kernel has them, so a MEMORY_DIRTY call could ask it later. For the files programs map today (a link's output, a few MiB), writing all of it back is cheap.
- **Only as far as the file went.** POSIX says writing past the end of a file into a mapping does not lengthen the file: on Linux the page past the end is there, but its bytes never reach the file. `file_bytes` is how much the mapping read in, and write-back stops there.
- **At exit, through atexit.** Unix writes shared mappings back when a process ends, however it ends. Here only a normal exit (`exit`, returning from `main`) does. A program killed by a signal loses what it didn't `msync`, and so does one that calls `_exit`, which is meant to skip everything.

## J29: not yet, and why

J29 is "programs loaded page by page from their file (not whole, not twice in memory)". Today the shell reads the program file into a memory object, and the kernel copies each page of it into fresh frames for the new process. For rustc's 182 MB that is 364 MB at the moment of loading.

The obvious fix is for the kernel to map the object's frames straight into the process for its read-only segments: code and constants, most of the file. That removes the copy, but it shares frames between the process's code and an object that someone else made and filled. The shell made it. The file service mapped it writable to fill it (READ_INTO). Either may still hold a writable handle or mapping when the program starts. Then another program could change this one's code while it runs. That breaks the rule the kernel keeps everywhere else, that no page is both writable by anyone and executable by anyone ("W^X across processes", as `map_memory` in process.rs puts it).

Doing it safely means **sealing**: a memory object that can be made read-only for good, its writable mappings gone and no handle left that could map it writable, checked by the kernel before it shares frames. That is a new kernel concept with its own protocol, and the shell, init and posix_spawn all hand objects along. It deserves its own step and its own design. The work list keeps J29, with that written next to it.

Loading page by page from the file itself goes further still, and needs the pager protocol described under Design decisions.

## Bugs

- **My own test, wrong by one letter:** the exit check looked for `x` at offset 9 of "by exit" written at 3. That's `t`; `x` is at 7. The code was right; the first boot said "failed: and what it wrote is in the file". I read atexit.c and dup before I counted the letters.
- **A mutation that didn't run:** replacing the `pthread_once` call left `register_write_all_back` unused, and `-Werror` stopped the build, so no test ran. I redid it by emptying the function instead.

## How I checked it

- `pages.c`, now 77 checks (part 1's 54 and 23 new), on x86_64 and AArch64:
  - `pwrite` and `pread` at an offset leave the descriptor's place where it was;
  - `pread` on a pipe is ESPIPE, a negative offset EINVAL;
  - a shared mapping holds the file's bytes and then zeros;
  - changes are not in the file before `msync` and are after it;
  - a byte written past the end of the file doesn't make the file longer;
  - a change after the descriptor was closed reaches the file at `munmap`;
  - a mapping from one page into the file goes back to its place;
  - `msync` of what is no longer mapped is ENOMEM;
  - a private mapping is never written back;
  - a child that maps a file, writes and exits without unmapping leaves its bytes in the file.
- Mutations, each put back:
  - write-back doing nothing: 4 checks fail (msync, munmap after close, the offset mapping, exit);
  - the exit handler never registered: only the exit check fails.
- `make test64`: all passed, the first time, in 32 minutes.

## What is still missing

- **J29** (above): sealing, then sharing a program's read-only segments with its file's object; later, paging from the file service.
- **Real sharing between programs** mapping the same file.
- **Dirty-page tracking**, so only changed pages are written back.
- **Write-back when a program is killed**, not only when it exits.

## What I learned

- **"Shared" promises two things**: writes reach the file, and every mapper sees every other's writes at once. Programs lean hard on the first and rarely on the second. Giving the first without the second is a real, useful point on that line, as long as the header says which one it is.
- **Sharing frames is a security decision, not only a memory one.** The cheapest fix for J29 would have opened the hole the kernel's W^X rule exists to keep shut.
- **Count the letters.** A test can be wrong as easily as the code it tests, and a failing test is a question, not an answer.
- **C:** `pthread_once` for registering an exit handler exactly once, even when two threads make their first shared mapping together. A static flag would race.

## Testing note

**1. The PC**

```
make run-disk
make run64
```

Close QEMU before `make run-disk`. It puts the new `pages` on the disk.

| Type | You should see |
|---|---|
| `with programs pages` | `pages: N copies went in, M were refused`, then `pages: 77 checks, all passed`; between them a second `pages` started "for a program" exits with code 0 (the child that writes through a mapping and exits) |
| `ls` afterwards | no `p.file` or `p.exit`: the checks clean up after themselves |

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
