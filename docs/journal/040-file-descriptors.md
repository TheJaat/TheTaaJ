# File Descriptors: Where an Open File Lives When Nobody Keeps It

*Journal entry #40 · `open`, `read`, `write`, `lseek`, `close`, `unlink` and `errno` for C programs — on a system whose kernel has no files and whose file service remembers nothing, so the "open file" turned out to live in the program itself*

A C program can now do what C programs mostly do:

```c
int file = open("notes", O_RDONLY);
if (file < 0) {
    fprintf(stderr, "cfile: %s: %s\n", "notes", strerror(errno));
    return 1;
}
char piece[512];
ssize_t count;
while ((count = read(file, piece, sizeof piece)) > 0)
    write(STDOUT_FILENO, piece, (size_t)count);
close(file);
```

```
taaj:/> with files cfile check
files: 48 checks, all passed
taaj:/> with files cfile write cnote made by c
cnote: 10 bytes
taaj:/> with readonly cfile cat cnote
made by c
taaj:/> cfile cat cnote
cfile: cnote: this program was not given the files
cfile exited with code 1
```

This is step 5 of the seven towards a C library: (1) toolchain and crt0 — 36; (2) standard error — 37; (3) memory and strings — 38; (4) `printf` — 39; (5) **file descriptors — this entry**; (6) `stdio` on files; (7) a real program and the recipe.

---

## What a descriptor is supposed to be

A *file descriptor* is a small whole number a program holds instead of "the thing I am reading". `open` returns one; `read`, `write` and `close` take one. 0, 1 and 2 exist from the start: standard input, output, error.

On Unix the number is an index into a table **the kernel keeps for the process**. Each entry points at an "open file" object that remembers, above all, *where the program has got to* — the offset. `read(fd, buf, 100)` means "the next 100 bytes", and the kernel knows what "next" means.

So I went looking for where that table should go here, and found there was nowhere on the other side of the fence to put it:

- **The kernel** has no idea what a file is. It has channels, memory objects and processes (that is what a microkernel means).
- **The file service** — which does know what a file is — *remembers nothing between requests.* I designed it that way in entry 13 and kept it that way through the disk and directories. Every request names the whole path and the offset:

```
READ   "docs/notes"  from 1024     →  up to 1023 bytes
WRITE  "docs/notes"  at   1024     →  the new size
STAT   "docs/notes"                →  file, 4096 bytes
```

There is no "open" request, because there is nothing for one to do. A service with no memory of its clients cannot be confused by a client that crashes, and can itself crash and restart without anyone losing their place.

Which leaves exactly one place for "where has this program got to": **the program**.

```
        Unix                                    TheTaaJ
 ┌───────────────┐                       ┌──────────────────────────┐
 │    program    │  fd 3                 │         program          │
 └───────┬───────┘                       │  fd 3 ─► path  "/notes"  │
 ════════╪════════ kernel                │         offset  1024     │
 ┌───────▼───────┐                       │         flags   O_RDONLY │
 │ fd table      │                       └────────────┬─────────────┘
 │  3 ─► offset  │                            READ "/notes" from 1024
 │       inode   │                       ┌────────────▼─────────────┐
 └───────────────┘                       │  file service (no memory) │
                                         └──────────────────────────┘
```

## The table

```rust
enum Open {
    Input,
    Output,
    Error,
    File(File),
}

struct File {
    /// From the root, with the '/' in front: what it meant when it was
    /// opened, whatever directory that was said in.
    path: String,
    offset: u64,
    flags: c_int,
}

struct State {
    /// The descriptors: the number is the index.
    table: Vec<Option<Open>>,
    input: taaj::Input,
    files: Option<Option<Files>>,
}
```

The table starts as `[Some(Input), Some(Output), Some(Error)]` — descriptors 0, 1 and 2 — and that one line is why `write(1, …)` and `write(3, …)` can be the same function. A slot that is `None` is a free number.

`Option<Option<Files>>` looks like a stutter and is not. The outer `Option` is *"have I asked for the file service yet?"*; the inner is *"and was I given one?"*. A program that never opens a file never asks.

With that, most of the calls are short enough to say in a sentence:

| Call | What it does here |
|---|---|
| `open` | Asks the service what the path is (`STAT`); makes or empties the file if the flags say so; writes `path, 0, flags` into the lowest free slot |
| `read` | Asks for bytes of `path` from `offset`; copies as many as were wanted; `offset += count` |
| `write` | Sends bytes for `path` at `offset` (or "at the end", with `O_APPEND`); `offset += count` |
| `lseek` | **Changes a number.** Only `SEEK_END` talks to the service, to learn the size |
| `close` | Sets the slot to `None`. The service is not told — there is nothing it would do with the news |
| `unlink` | Asks the service to remove the path |

`lseek` is the one that made the design click for me. On Unix it is a system call. Here it is arithmetic:

```rust
match from.checked_add_signed(offset).filter(|&place| i64::try_from(place).is_ok()) {
    Some(place) => {
        file.offset = place;
        place as i64
    }
    None => fail(EINVAL),
}
```

(`checked_add_signed` adds a signed number to an unsigned one and gives `None` if the result would be below zero or overflow — "before the start of the file" becomes an ordinary `None`.)

## `open`, in full

```rust
let made_ready = match files.stat(&file.path) {
    Ok((Kind::Directory, _)) => return fail(EISDIR),
    Ok((Kind::File, _)) if flags & (O_CREAT | O_EXCL) == O_CREAT | O_EXCL => return fail(EEXIST),
    // Writing nothing over a file leaves it empty.
    Ok((Kind::File, _)) if flags & O_TRUNC != 0 && file.writes() => files.write(&file.path, &[]).map(|_| ()),
    Ok((Kind::File, _)) => Ok(()),
    // Adding nothing to a file that is not there makes it.
    Err(FileError::Refused(taaj::files::Status::NotFound)) if flags & O_CREAT != 0 => {
        files.append(&file.path, &[]).map(|_| ())
    }
    Err(error) => Err(error),
};
```

A `match` with *guards* (`if …` after a pattern): the arms are tried top to bottom, and an arm is taken only if its pattern matches **and** its guard is true. It reads like the table in a manual page: a directory — refused; a file and you insisted on a new one — refused; a file and you asked to empty it — emptied; a file — fine; nothing there and you said to make it — made; anything else — whatever the service said.

Two details in the flags:

- **Reading/writing is a number, not bits.** `O_RDONLY` is 0, `O_WRONLY` 1, `O_RDWR` 2. You cannot test "is it readable" with `flags & O_RDONLY` (that is `& 0`, always false) — a famous trap. It is the lowest two bits taken together: `flags & O_ACCMODE`.
- **The third argument.** `open(path, flags)` and `open(path, flags, 0644)` are both correct C: the permissions are passed only when a file might be made. "Two arguments or three" is `...` again, so `open` itself is five lines of C (`libc/c/fcntl.c`) that call the Rust function with the two that matter. The mode is accepted and ignored: files here have no permissions.

## Who may open a file at all

On Unix, whether `open` succeeds depends on the file's permission bits and who you are. Here it depends on **what the program was handed when it started** (entries 34 and 35): a program asks the registry for the service named `files`, and the shell decided, when it started the program, what that name leads to — the channel that can change files, the one that can only read, or nothing.

A C program I just compiled is not on any of the shell's lists, so by default it gets nothing:

```
taaj:/> cfile cat welcome
cfile: welcome: this program was not given the files
taaj:/> with readonly cfile cat welcome
Hello from the host                  (a file the test disk starts with)
taaj:/> with readonly cfile write cno x
cno: the files are read-only here
taaj:/> with files cfile write cnote made by c
cnote: 10 bytes
```

Those are `EACCES`, success, `EROFS`, and success. I did not have to *build* any access control for C programs. The capability design was already there; `open` only had to report truthfully what it ran into. (For a descriptor opened to write, `open` asks the service "is this channel read-only?" straight away, so the program hears at `open`, where it expects to, and not at its first `write`.)

## `errno`

A C function has one return value and it is usually spoken for — `read` returns a count. So failure is a value that could not be an answer (`-1`), and the *reason* goes in a variable everybody agrees on.

```rust
#[cfg_attr(not(test), no_mangle)]
#[allow(non_upper_case_globals)]
pub static errno: AtomicI32 = AtomicI32::new(0);
```

To C this is `extern int errno;`. In Rust I made it an `AtomicI32`: in memory an atomic integer *is* an `int`, so C reads and writes it as one, and Rust can store to it without `unsafe` (a plain `static mut` needs `unsafe` at every touch).

One generic helper sets it and produces the -1 in whatever type the function returns:

```rust
fn fail<T: From<i8>>(code: c_int) -> T {
    errno.store(code, Ordering::Relaxed);
    T::from(-1)
}
```

`T: From<i8>` means "any type that can be made from an `i8`" — `c_int`, `isize`, `i64` all can — so `return fail(EBADF)` works in `close` (returns `int`), `read` (returns `ssize_t`) and `lseek` (returns `off_t`) alike. The compiler picks `T` from the function's return type.

The numbers are Linux's. They have to be *some* numbers, and then a program that prints one is understood by anyone. The words are this system's own — where the file service already had words for a thing, `strerror` uses the same ones, so a C program's complaint reads like `cat`'s:

| errno | Number | `strerror` says |
|---|---|---|
| `ENOENT` | 2 | no such file or directory |
| `EBADF` | 9 | not an open descriptor, or not open for that |
| `ENOMEM` | 12 | out of memory |
| `EACCES` | 13 | this program was not given the files |
| `EEXIST` | 17 | there is already something with that name |
| `EISDIR` | 21 | is a directory |
| `EINVAL` | 22 | not a valid argument |
| `EMFILE` | 24 | too many open descriptors |
| `ESPIPE` | 29 | a stream has no position to move |
| `EROFS` | 30 | the files are read-only here |

(and `EIO`, `EFAULT`, `ENOTDIR`, `EFBIG`, `ENOSPC`, `ENOTEMPTY`, `ETIMEDOUT`). `malloc` now sets `ENOMEM`, and `perror` exists.

## What the design costs

Putting the open file in the program is not free, and the price is specific. A Unix descriptor is tied to **the file**. Mine is tied to **the name**.

| On Unix | Here |
|---|---|
| Rename an open file: the descriptor still reads it | The descriptor now reads whatever has the old name — or fails with `ENOENT` |
| Remove an open file: it lives until the last `close` | It is gone at once; the next `write` to the descriptor *makes a new file* of that name |
| `O_CREAT \| O_EXCL` is atomic: exactly one of two racing programs wins | "Is it there?" and "make it" are two requests; two programs can both win |
| `read` of 1 byte costs one system call | It costs a round trip to the file service, which sends back up to 1023 bytes of which 1 is kept |

None of these bit anything in this entry's tests. All of them are real, and the first two are why real systems have an "open file" object on the *server's* side. If this system ever needs them, the answer is a file service that hands out a handle per open file — a channel, most likely, which the kernel already knows how to count and close. That is a different file service, and not this step.

The last row is the practical one, and it is what the next step is for: `stdio`'s `FILE` exists precisely to put a buffer in front of `read` so that `getc` is not a round trip per byte.

## The design, in one table

| Decision | What I chose | What I rejected, and why |
|---|---|---|
| Where the descriptor table lives | In the program, in the C library | In the kernel (it has no files); in the file service (it would have to remember clients, and survive their crashes and its own) |
| What an open file is | Path + offset + flags | A handle from the service per open file — the right answer for rename/remove semantics, and a redesign of the service |
| The path kept | Resolved to the root at `open` | The name as given — then a later `chdir` (when there is one) would silently move every open file |
| Descriptors 0, 1, 2 | Ordinary entries in the same table | Special cases in `read`/`write` — and then `close(1)` could not work |
| Number returned by `open` | The lowest free | The next unused — simpler, but C promises lowest, and old code relies on it (`close(0); open(…)` to replace standard input) |
| Limit | 32 open at once | No limit — a leak then grows until memory runs out, far from its cause |
| No access to files | `EACCES`, at `open` | Failing at start-up — a program that never opens a file should not care |
| Read-only channel | `EROFS`, at `open` for writing | At the first `write` — legal, but later than a program expects |
| `errno` numbers | Linux's | My own — nothing gained, and every debugging habit lost |
| `errno` storage | `AtomicI32` named `errno` | `static mut` — `unsafe` at every use, for the same bytes |
| `open`'s third argument | C wrapper taking `...`, mode ignored | Declaring `open` with three fixed arguments in Rust — works on this machine's calling convention, and is a lie in the header |
| Short reads | `read` returns what one reply held, at most what was asked | Looping until the buffer is full — C allows short reads, and a loop hides where the end is |

## The bugs

The library's code ran correctly the first time it booted: 48 of 48. The two mistakes were in my tests, and both would have been easy to miss.

**A capital letter that was never typed.** The automated test types its commands as QEMU key names. I wrote `… write cnote made by C`, and the file came out 8 bytes, not 10. My key table has no entry for capitals, so `C` was sent as a key name QEMU does not know and silently dropped: the shell was sent `made by ` and the file held `made by\n`. The host-side comparison of the file caught it; the log line `cnote: 8 bytes` alone would have looked plausible.

**An expectation that could not fail.** I wanted to check that a C program reads standard input, with `cfile cat < long | wc`, expecting `300 lines, 1200 words`. It passed — and I then noticed the same line was already in the log from a test three entries old. The check would have passed with `cfile` deleted. I replaced it with `cfile cat < long > clong2` and a byte-for-byte comparison of `clong2` on the host.

(And a small thing the compiler caught: I had declared `O_RDWR` in the Rust code and never used it — because "both" is simply "not read-only and not write-only". The warning made me write that down as a comment instead.)

## How I checked it

**Automated** (`make test64`):

- **Host:** 4 tests of `errno.rs` (`make libc-test`): every message is a C string with exactly one zero, at its end; no number twice and no two messages alike; `strerror` returns them, and "unknown error" for a number it does not know; the numbers match Linux's.
- **In the booted system, BIOS and UEFI:**
  - `with files cfile check` → `files: 48 checks, all passed`. The checks: every failure's `errno` (`ENOENT`, `EBADF` six ways, `EISDIR`, `EINVAL`, `EEXIST`, `ESPIPE`, `EMFILE`, `ENOMEM`); lowest-free numbering and reuse; partial reads, the end, and the end again; `lseek` from all three places; two descriptors on one file keeping separate offsets; writing in the middle; `O_APPEND` ignoring `lseek`; writing past the end leaving zeros; `O_TRUNC`; 5000 bytes written in one call and read back 700 at a time; exactly 29 more descriptors available and all returned after closing; `unlink`; `errno` untouched by success.
  - `cfile cat welcome` (no grant) → `this program was not given the files`.
  - `with readonly cfile cat nofile welcome` → the complaint for `nofile`, and still `welcome`'s contents.
  - `with readonly cfile write cno x` → `cno: the files are read-only here` (through `perror`), and the host confirms no file `cno` exists.
  - `with files cfile write cnote made by c` → the host reads `cnote` and it is `made by c`.
  - `with readonly cfile cat long > clong` and `cfile cat < long > clong2` → the host compares both with the 300-line original, byte for byte. The first is `open`/`read`; the second is descriptor 0.
  - The host confirms `cfile.tmp` is not on the disk afterwards.

**Not tested:**

- Any of the costs in the table above: renaming or removing a file while it is open; two programs racing on `O_EXCL`.
- The file service stopping or stalling under a C program (`EIO`, `ETIMEDOUT`), and a full disk (`ENOSPC`).
- `EFAULT` (null pointers) and a path that is not UTF-8.
- `close(0)`/`close(1)` followed by `open` taking their number.
- `read(0, …)` with no standard input given: it returns 0 at once. A C program cannot read the keyboard.
- A file bigger than a megabyte or so.

## What is still missing

- No `stat`/`fstat`, `mkdir`, `rmdir`, `rename`, `dup`/`dup2`, `pipe`, `isatty`, `getcwd`, `chdir`, `access`, `ftruncate`.
- No directories from C at all (`opendir`/`readdir`).
- Descriptors follow names, not files; `O_EXCL` is not atomic (both above).
- Every `read` is a round trip, however small. `stdio` buffering is the next step.
- A `write` that fails part-way through a large buffer returns -1 though some bytes were written.
- The keyboard is not a standard input for anything yet.
- `errno` is one variable: correct for one thread, which is all a program has.

## What I learned

- **"Where does this state live?" is the first design question, and the answer was decided two dozen entries ago.** A stateless file service was chosen for crash recovery. Its consequence — the open file lives in the client — arrived today, uninvited and unavoidable.
- **A file descriptor is two ideas wearing one number**: an index into a table, and the offset that table remembers. Unix puts both in the kernel; nothing says they must be there.
- **`lseek` is not I/O.** It is the clearest proof that an offset is just a number someone has to hold.
- **Being tied to the name instead of the file is a real difference**, and I now understand why Unix has inodes: so that "the file" is a thing apart from what it is called.
- **`O_RDONLY` is zero.** Access mode is a two-bit number, not flags.
- **`Option<Option<T>>` is two questions**, and it is fine for a type to say so.
- **A generic return (`fn fail<T: From<i8>>`)** lets one helper serve functions with different return types; the caller's signature picks `T`.
- **A test must be able to fail.** One of mine could not, and it passed. I only saw it because I asked what would happen if the thing under test were removed.

---

## Testing note

```
make run64
```

The disk `make run64` uses starts with only the programs on it, so the first line makes a file to read.

**Without the files, and with them**

| Type | You should see |
|---|---|
| `echo hello from a file > hi` | Nothing |
| `cfile cat hi` | `cfile: hi: this program was not given the files`, then `cfile exited with code 1` |
| `with readonly cfile cat hi` | `hello from a file` |
| `with readonly cfile cat nofile hi` | `cfile: nofile: no such file or directory`, then `hello from a file`, then `cfile exited with code 1` |
| `with readonly cfile cat /` | `cfile: /: is a directory`, then `cfile exited with code 1` *(not in the automated test, which checks the same refusal inside `cfile check`)* |

**Writing**

| Type | You should see |
|---|---|
| `with readonly cfile write cno x` | `cno: the files are read-only here`, then `cfile exited with code 1` |
| `with files cfile write cnote made by c` | `cnote: 10 bytes` |
| `cat cnote` | `made by c` *(the `cat` is not automated; the host reads the file)* |
| `with files cfile write cnote shorter` | `cnote: 8 bytes` *(not in the automated test)* |
| `cat cnote` | `shorter` — the old contents are gone: `O_TRUNC` *(not in the automated test)* |

**Standard input, and output to a file**

| Type | You should see |
|---|---|
| `cfile cat < hi > hi2` | Nothing — descriptor 0 in, descriptor 1 out, and no files asked for |
| `with readonly cfile cat hi > hi3` | Nothing |
| `wc hi hi2 hi3` | The same counts three times: `hi: 1 line, 4 words, 18 bytes`, and likewise `hi2` and `hi3` *(not automated in this form; the automated test does both with a 300-line file and the host compares the copies)* |
| `echo down a pipe \| cfile cat` | `down a pipe` *(not in the automated test)* |
| `cfile cat` | Nothing, and the prompt at once: there is no standard input to read *(not in the automated test)* |

**The checks**

| Type | You should see |
|---|---|
| `with files cfile check` | `files: 48 checks, all passed` |
| `ls` afterwards | No `cfile.tmp` |
| `with readonly cfile check` | `cfile: check needs to make a file (with files cfile check): the files are read-only here`, then `cfile exited with code 2` *(not in the automated test)* |
| `cfile` | `usage: cfile cat [FILE...] \| write FILE WORD... \| check`, then `cfile exited with code 2` |

**On the host**

```
make libc-test
```

now runs three things: 10 string tests, 4 `errno` tests, and `format: 123 checks, all the same as the host's printf`.

**A program of your own** *(not tested: please try it)* — on the host:

```
cat > libc/programs/mine.c <<'EOF'
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>
int main(int argc, char **argv)
{
    if (argc != 2) {
        fprintf(stderr, "usage: mine FILE\n");
        return 2;
    }
    int file = open(argv[1], O_RDONLY);
    if (file < 0) {
        fprintf(stderr, "mine: %s: %s (errno %d)\n", argv[1], strerror(errno), errno);
        return 1;
    }
    printf("%s is %ld bytes\n", argv[1], lseek(file, 0, SEEK_END));
    close(file);
    return 0;
}
EOF
make cprogs64 C_PROGRAMS=mine
tools/tfstool-rs/target/release/tfstool put build/taaj-disk.img mine build/c/mine
```

(with QEMU closed), then `make run64`:

| Type | You should see |
|---|---|
| `with readonly mine hi` | `hi is 18 bytes` |
| `with readonly mine nothing` | `mine: nothing: no such file or directory (errno 2)` |
| `mine hi` | `mine: hi: this program was not given the files (errno 13)` |

Delete `libc/programs/mine.c` afterwards, and `rm mine` in the OS.

**From the host** — with QEMU closed:

```
tools/tfstool-rs/target/release/tfstool get build/taaj-disk.img cnote
```

prints what was last written to it.

**Cleaning up**, in the OS: `rm hi hi2 hi3 cnote`.

Automated: `make test64`.
