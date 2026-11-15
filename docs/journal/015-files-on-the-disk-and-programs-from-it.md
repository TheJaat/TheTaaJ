# Files on the Disk, and Programs Loaded From It

*Journal entry #15 · TaajFS in the file service; `process_spawn_image`*

Entry #13 gave TheTaaJ a file service; entry #14 gave it a disk. This entry joins them — files that survive a reboot, in a format that survives a power cut — and then takes the step that changes what the system *is*: a program no longer has to be compiled into the kernel image. It can be a file.

---

## Part 1: TaajFS

### A format I already had

TaajFS is the project's own file system format, designed in the 32-bit days: implemented in C in the old system, and in Rust in a host tool, `tfstool`, which creates and inspects disk images. A script, `crosscheck.sh`, holds the two implementations **byte-identical**: the same operations must produce the same image.

For this step the Rust code moved out of the tool into a shared `no_std` crate, `taajfs/`, used by both the host tool and the file service. The tool keeps only its command line and its image-file device on top.

It talks to storage through one small trait, `BlockDevice` — read a block, write a block. On the host that is a file or a vector of bytes; in the OS it is the disk driver. The file system code is identical.

One difference from the C version is deliberate. C gets the on-disk layout from `struct` definitions and trusts the compiler's layout. The Rust code reads and writes every field **at an explicit offset, in little-endian order**. It is more typing; in exchange the layout is written down rather than implied, and is correct on a big-endian machine too.

### Copy-on-write: never overwrite what is live

The danger for any file system is a power cut **in the middle of a change**. If a file's blocks are half-overwritten when the power goes, the file is neither old nor new: it is corrupt. Classic file systems repair this with a checking tool at boot, or avoid it with a journal.

TaajFS avoids it by construction, with three rules:

1. **A block reachable from the committed superblock is never written.**
2. A change writes **new** blocks — the new data, a new inode, a new directory — into free space.
3. The change is **committed by writing one block**: a new superblock, into whichever of the two superblock slots is *not* the live one.

Until step 3 the disk still describes the old state, completely. After it, the new state, completely. There is no moment in between. At mount, the valid superblock with the higher generation number wins; a superblock torn by a power cut fails its checksum, and the other slot — the previous state — is used.

There is no free-space map on disk either: it is rebuilt at mount by walking the tree. Nothing on disk can disagree with the tree, because only the tree exists.

The limits, honestly: 4 KiB blocks; names up to 28 bytes; one flat directory; files just under 4 MB (an inode holds 1020 direct block pointers); volumes up to 65,536 blocks.

### Testing a power cut

The crate includes a test disk that **cuts the power after N writes** — every later write fails. The tests run an operation with the power cut after write 1, after write 2, and so on through every write the operation makes, and each time remount and require: *either the complete old state or the complete new state*. Never anything else.

## Part 2: the file service on the disk

### One service, two stores

The service's `serve` function became generic over a trait, `Store`, with two implementations:

- `RamStore` — the table from entry #13;
- `DiskStore` — a TaajFS volume on any block device.

The protocol and the service loop did not change. Clients cannot tell which store is behind it, except that one of them remembers.

### One request, one commit

Each request is one TaajFS commit. Creating a file and filling it is one commit; truncating is one. So **a request that is refused, or interrupted by a crash, changes nothing** — and the reply is sent only *after* the commit is on the disk. When a client hears "ok", the data is durable.

This too is tested by cutting the power at every write of an append-then-truncate sequence.

### Mounting, and never formatting

`fs` mounts the disk through `blk`. If there is no disk, or no TaajFS volume on it, it falls back to memory and says so. **It never formats a disk.** A file service that "helpfully" initialises a disk it does not recognise is one that destroys someone's data the day it is pointed at the wrong disk. Creating a volume is an explicit act with `tfstool` on the host.

### Who may write to the disk?

`blk` now serves **two channels**: read-write for `fs`, read-only for the shell. If the shell (or anything started from it) could write raw sectors, it could corrupt the file system under the service that owns it. The driver enforces the difference per channel — which is why `disk write` in the shell now answers `the disk is read-only`.

### What restart means now

When `fs` crashed in entry #13, the files were gone. Now a restarted `fs` mounts the same disk and finds every committed change. The combination — supervised restart (entry #10) plus a copy-on-write disk format — is the first time a *stateful* service in TheTaaJ can crash and come back whole.

## Part 3: programs from the disk

### `process_spawn_image`

Until now `process_spawn` took a *name*, and the kernel looked it up among programs compiled into its own image. The new call, `process_spawn_image`, takes the program as a **memory object** holding an ELF file.

Two details:

- It has seven arguments, and system calls pass six in registers. So they are passed as a small structure in memory, read with the same checked copy as any user pointer.
- **The kernel copies the file out of the memory object once, then checks and loads the copy.** The caller still has the object mapped and could change the bytes *after* the kernel validated them and *before* it used them — a "time of check to time of use" attack. Working from a private copy makes the check and the use see the same bytes.

### Who reads the file?

The shell asks init to run a program. Should init read the file from the file service? No: **init supervises the file service, so init must never wait on it.** If `fs` hung, init would hang with it — and then nothing could restart `fs`. A supervisor may not depend on what it supervises.

So the *shell* reads the file into a memory object and sends that to init. If `fs` hangs, the shell hangs; init is unaffected and stays able to act.

### A program file is untrusted input

A file on a disk could be anything. Running it has to be safe **by construction**:

- At this step init gave the new process **only the screen** — whatever the file contains, it could do no more than print. (Entry #16 widens this, carefully.)
- The loader was tightened, because its input is no longer something I compiled:
  - **a leak fixed**: a frame was not freed if mapping it failed;
  - a program's segments may total at most **64 MiB** — a few bytes of header can otherwise demand gigabytes of zeroed memory;
  - segments must lie **below the mapping area**, clear of memory objects and the stack;
  - process names became owned strings — they used to be `&'static str`, which only a compiled-in name can be.
- A file that is not a program is refused with a new error, `NotExecutable`.

New tests feed the ELF parser **every truncation** of a good file and **every single-bit flip** of its header. None may panic; each is either refused or still valid.

### `primes`

The first program that exists *only* as a file: it prints the primes below 50. It is in no kernel image. The boot test puts it on the disk with `tfstool`, runs it, and also tries to run a text file, a truncated copy of a program, and a name that does not exist — each must be refused with the right message.

## How I checked it

- **Host**: the TaajFS tests (format, volume, power cuts), the store tests, `crosscheck.sh` against the C implementation.
- **Boot tests**: the test disk is made with `tfstool`, containing a file for the OS to read. The OS writes another and removes one. **After QEMU exits, `tfstool` reads the image**: the written file must be there with the right contents, the removed one gone.
- **A second boot of the same image** must find the files.

## What I learned

- **Atomicity comes from the order of writes**, not from repair afterwards: make the last write the one that switches states.
- **Validate, then use — the same bytes.** Copy untrusted input out of shared memory before checking it.
- **Draw the dependency graph of your supervisors.** init must not wait on anything it is responsible for restarting.
- **Refusing to act is a feature**: never format; never run what is not a program.

---

## Testing note

```
make run64
```

In the terminal log:

| Line | Meaning |
|---|---|
| `[fs] ready: TaajFS on the disk, N of 4096 blocks free; a cache of 256 blocks` | The file service mounted the volume (the cache is entry #18's) |
| `[init] started primes for the shell, from a file` | A program was loaded from the disk |

In the QEMU window:

| Type | You should see |
|---|---|
| `ls` | The programs on the disk — `ls`, `cat`, `write`, `append`, `rm`, `primes`, `forever` — with sizes |
| `run primes` | `2 3 5 7 … 47`, then `primes exited with code 0` |
| `write note remember me` | `note: 12 bytes` |
| `run note` | `error: note is not a program` |
| `run nothing` | A refusal: no such file or program |
| `disk write 5 x` | `disk: sector 5: the disk is read-only` |

**Persistence** — the real test:

1. `write keep this survives`
2. Close the QEMU window.
3. `make run64` again.
4. `cat keep` → `this survives`

**From the host**, with QEMU closed:

```
tools/tfstool-rs/target/release/tfstool ls build/taaj-disk.img
tools/tfstool-rs/target/release/tfstool get build/taaj-disk.img keep
```

should list the same files and print the same text: the OS and the host tool agree on the format.

**Crash recovery** is not yet wired to a key the way F12 is for the keyboard driver; the automated evidence is the power-cut tests in `cd taajfs && cargo test` and `cd fileserver && cargo test`.

Automated: `make kernel64-test`, `make boot64-test`, `make boot64-uefi-test`.
