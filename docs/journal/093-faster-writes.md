# Faster writes: a commit per megabyte, not per kilobyte

*Journal entry #93 · Era 6, step "faster writes". Writing a megabyte to a file on TheTaaJ took 2.4 seconds whichever way it was written: about 1,000 commits of 1 KB each. Now:
- a write bigger than a message travels to fs in a memory object, one commit per MiB;
- fs adds all the messages waiting on an output stream in one commit;
- a file `fopen` opens has a 64 KB buffer;
- `cp` copies a megabyte per commit.

The same megabyte now takes 0.24 s with big writes or stdio, a stream 0.39 s, and `cp` 0.36 s instead of 4.44 s.*

## Why

Every change on TaajFS is copy-on-write and committed before fs answers. That costs a new data block, a new inode, new directory blocks up to the root, and the superblock, whether the change is one byte or a megabyte. But a request carried at most one message, about 1 KB. So `write(fd, buffer, 65536)` became 64 requests and 64 commits. stdio flushed 4 KB at a time, so 4 commits. A program's output into a file came as stream messages of 1 KB, one commit each. And `cp` appended each 1 KB piece it read.

TheTaaJ now builds things on itself (TCC, make, the C library), and the compilers' output goes to files, so this matters every day. It was the next step of Era 6.

## How it fits together

```
 libc write(fd, 1.5 MiB)            stdio (64 KB buffer)       prog > file (a stream)
        │                                  │                           │ DATA messages, ~1 KB each
 taaj::files::write_at                     │                           ▼
   > one message's worth?  ── no ──► WRITE (as before)        fs: take_from
   yes: memory_create, copy in                                   every message waiting (up to 64)
        WRITE_FROM [from][length]  ×1 per MiB                    gathered -> ONE store.write
        │ + the object (READ right)                                     │
        ▼                                                               ▼
 fs: map the object read-only -> fileserver::serve_from -> store.write -> ONE commit
```

**The protocol** (`fileserver/src/lib.rs`):
- `WRITE_FROM` (op 15) works like `READ_INTO`, the other way round. The request's second handle is a memory object. The data field says where the bytes are in it and how many: `[from: u64][length: u64]`. `number` is the file offset, or `APPEND`.
- At most `WRITE_FROM_MAX` = 1 MiB per request. fs does nothing else meanwhile, and init stops a service that hasn't answered for 4 seconds.
- `serve_from` checks everything before touching the store: the channel may change files, the range lies inside the object (with `checked_add`, so `u64::MAX` can't wrap round), and the length is at most 1 MiB. A refused request changes nothing.

**The client** (`user/src/files.rs`): `write_at` sends WRITE as before when the data fits in one message. Otherwise `write_through_memory` creates an object, copies the bytes in, and sends one `WRITE_FROM` per MiB, each with a duplicate of the object holding only the READ right. libc's `write()` to a file goes through here, so every C program gets it.

**Streams** (`user/src/bin/fs.rs`): `take_from` used to take one message and write it. Now it takes every message already waiting on that stream, up to 64 (a channel holds 32), gathers their bytes, and makes one write. A writer that closes after sending still has everything it sent written first.

**stdio** (`libc/c/stdio.c`): `fopen` gives the file a 64 KB buffer (`FILE_BUFFER`). `fdopen`, whose descriptor may be a pipe or the screen, keeps `BUFSIZ`: there a big buffer only holds output back.

**`cp`** gathers what it reads and appends a megabyte at a time.

## Design decisions

| Decision | Chosen | Rejected, and why |
|---|---|---|
| How big writes travel | A memory object, like READ_INTO | Many messages in one "transaction": fs would have to hold half a change between requests, and a client that dies in the middle would leave it hanging |
| When fs commits | Still before answering: every write that is answered is on the disk | Write-back: answering first and committing later (on a timer, or when idle) makes 1 KB writes fast too, but a write's error (disk full) could no longer reach the `write()` that caused it, and "answered means on the disk", which fs has promised since it first wrote to a disk, would go |
| The largest WRITE_FROM | 1 MiB | 4 MiB like READ_INTO: writing is slower than reading, since every block goes to the disk, and fs must answer others within init's 4 seconds |
| Gathering stream messages | What is already waiting, up to 64 | Waiting a little for more: a program that writes one line and then waits for its reader would wait for nothing |
| stdio's buffer | 64 KB for files, BUFSIZ for the rest | Raising BUFSIZ: programs size arrays by it, and output to the screen would come in big lumps |
| `cp` | Gather 1 MiB, then append | Read whole with READ_INTO: needs as much memory as the file |

## Bugs

None reached the tests this time. One test's assumption broke: `stdio_test.c` checked that "more than a buffer goes straight out, in one write" with 20,000 bytes, which was more than 4 KB. With 64 KB it now writes 8,198 items of 8 (65,584 bytes), chosen so the follow-up `fread` of 7-byte items still comes out exact (47,684 = 7 × 6,812).

## How I checked it

**Speed** (x86_64 in QEMU, 4 CPUs, `time` in TheTaaJ's shell; a scratch boot, `write-bench`, each writing about 1 MB on a fresh 32 MB disk):

| Write | Before | After |
|---|---|---|
| `write()`, 1 KB at a time (1,024 calls) | 3.64 s | 2.12 s |
| `write()`, 64 KB at a time | 2.41 s | **0.24 s** |
| stdio, 100-byte lines (`fprintf`) | 2.39 s | **0.25 s** |
| a stream: `pipes lines 100000 > d.txt` (1.1 MB) | 2.36 s | **0.39 s** |
| `cp d.txt e.txt` (1.1 MB) | 4.44 s | **0.36 s** |

The 1 KB case is still one commit per `write()`: POSIX says a write is done when it returns, and here "done" means committed. It got faster only because each 1 KB now takes one request instead of two (a WRITE carries a little less than 1 KB once the name is in it). I read the files back on the host: all five were byte for byte what was written.

**Tests:**
- `cargo test` in `fileserver`: 37 → 40 tests.
  - `writes_from_memory` runs on both stores: one request writes many blocks; an append and an overwrite from an offset in the object; past the object, longer than it, wrapping round, more than 1 MiB and the read-only channel are each refused and change nothing; a request that isn't WRITE_FROM is none of `serve_from`'s business.
  - `many_stream_messages_are_one_change`: five messages gathered, one commit (the change stamp goes up by exactly one).
- `make libc-test`: stdio's 124 checks with the new buffer.
- `pipes.c`: 102 → 108 checks. One `write()` of 1.5 MiB (two WRITE_FROMs), then 300 KB appended in 7,000-byte writes, read back byte for byte. 2,000 lines through stdio, the right size, the last line where it belongs. The test disk grew from 4 MB to 8 MB to hold them.
- `make pipe64-test` and `make aarch64-pipe-test` passed.

**Mutation experiments**, each undone:

| What I broke | What happened |
|---|---|
| `serve_from` takes the bytes from the object's start, ignoring `from` | 2 host tests failed: `writes_from_memory_in_memory`, `_on_disk` |
| fs keeps only the first of the gathered stream messages | 2 checks failed: "every line in the file once it ended", "O_APPEND kept what was there" |
| The client doesn't move the offset from one MiB to the next (same run) | 2 checks failed: "one write() of 1.5 MiB", and the append after it |

`make test64` passed, in 13 min 26 s. Its make-on-TheTaaJ part built the C library and 12 programs in 17.28 s on x86_64 and 21.03 s on AArch64. The same part took 41.56 s and 48.72 s in entry 092's test64 run earlier today, but that whole run was slow (31 min, the machine busy), so I don't count the difference as this step's.

I checked the testing note with a scripted boot: `cp` copied tccgen.c in 0.17 s, byte for byte. That boot showed one thing I didn't know: TheTaaJ's own shell sends `time`'s "took" line into a redirected file (`time cat f > g` puts it at the end of g). I left that alone; it belongs to the shell's `time`, not to writes. The note doesn't time the `cat` line because of it.

## What is still missing

- **Small writes are still one commit each.** A program calling `write()` 1 KB at a time, or the shell's `echo >> file` in a loop, pays a commit per call. The fix is write-back in fs: commit when idle or on a timer, flush on `fsync`. But then an error such as a full disk would have to wait for the next call or `fsync`, as on Linux. I'll decide that when something needs it.
- **A commit still rewrites the path up to the root.** Writing to `/a/b/c/file` rewrites four directories every time. TaajFS has no inode table that would let a file change without its directory changing.
- **TheTaaJ's shell sends `time`'s report into the command's redirection.** Found while checking the testing note; sh's `time` would use standard error.
- **Reading is unchanged:** READ_INTO for whole programs, about 1 KB per READ otherwise.

## What I learned

- **Measure first.** Five ways of writing a megabyte all took about 2.4 s, which said the cost was per request, not per byte. Without the "before" column, I would have been guessing which part to fix.
- **A fixed cost per operation is best fought by having fewer operations**, not by making each one cheaper. Gathering waiting stream messages was a dozen lines and made streams six times faster.
- **Copy-on-write with one superblock** makes a batch free to be atomic: a 1 MiB WRITE_FROM is all there or not there after a power cut, exactly like a 1 KB WRITE was.

**Rust, as used here:**
- **`(|| { ... })()`**: a closure called at once, so that `?` can be used inside `serve_from` and every early refusal lands in one `match` that writes the reply. It is the closure version of a `try` block, which Rust doesn't have yet.
- **`Option::filter` and `and_then` chains** for decoding: `request.data.get(..8).and_then(protocol::number)` is "the first 8 bytes, if there are 8, read as a number". There's no index that can panic and no length checked twice.
- **`checked_add`**: `start + length` from a client could overflow. `start.checked_add(length)` gives `None` instead of wrapping to a small number that would pass the bounds check. One of the host tests sends `u64::MAX` for exactly that.
- **`step_by`**: `(0..data.len()).step_by(WRITE_FROM_MAX)` gives the start of each piece. No counter to get wrong.

## Testing note

**1. The PC**

```
make run-disk
make run64
```

Close QEMU before `make run-disk`. `src/tcc/tccgen.c` (273,655 bytes) is one of the TCC sources the disk carries.

| Type | You should see |
|---|---|
| `time with files cp src/tcc/tccgen.c copy.c` | `copy.c: 273655 bytes`, then `took 0.xx s`: well under a second (0.17 s in my scripted boot) |
| `with files cat src/tcc/tccgen.c > cat.c` | nothing: cat's output, a stream, went into cat.c, gathered into a few commits. (Not under `time`: TheTaaJ's shell sends time's "took" line into the redirected file too) |
| `with files wc copy.c` and `with files wc cat.c` | `9197 lines, 32363 words, 273655 bytes` for each, the same as `with files wc src/tcc/tccgen.c` |

**2. AArch64**

```
make run-aarch64-disk
make run-aarch64
```

The same as part 1.

**3. The tests by themselves**

```
make test64
```

| Look for | Meaning |
|---|---|
| `pipes on TheTaaJ (x86_64): 108 checks ... passed` | big writes, stdio and streams, on the PC; the same for aarch64 |
| `test64: all passed` | every part |

Automated: make test64
