# Streams: A Buffer in Front of a Descriptor

*Journal entry #41 · `FILE`, `fopen`, `fgets`, `fread`, `fwrite`, `fseek`, `stdin`, and the three ways text can wait — the part of C's library that exists because asking the system for one byte is expensive, on a system where it is more expensive than usual*

A C program can now read a file the way C programs are actually written:

```c
FILE *file = fopen("notes", "r");
if (file == NULL) {
    fprintf(stderr, "cstdio: %s: %s\n", "notes", strerror(errno));
    return 1;
}
char line[128];
int number = 0;
while (fgets(line, sizeof line, file) != NULL)
    printf("%4d  %s", ++number, line);
fclose(file);
```

```
taaj:/> with files cstdio check
streams: 52 checks, all passed
taaj:/> echo hello from a file > hi
taaj:/> with readonly cstdio lines hi
   1  hello from a file
taaj:/> cstdio order
write was called second - printf was called first
```

That last line is the whole entry in one line of output, and I will come back to it.

This is step 6 of the seven towards a C library: (1) toolchain and crt0 — 36; (2) standard error — 37; (3) memory and strings — 38; (4) `printf` — 39; (5) file descriptors — 40; (6) **`stdio` on files — this entry**; (7) a real program and the recipe.

---

## Why a stream exists

Entry 40 ended on a cost. `read(fd, &byte, 1)` on this system is not a system call that copies a byte. It is:

1. make a reply channel,
2. send a message to the file service — "bytes of `/notes` from 4711",
3. be switched out; the file service is switched in, finds the file, reads the disk block (or its cache), sends back up to 1023 bytes,
4. be switched back in, copy **one** of those bytes out, throw away the other 1022,
5. close the reply channel.

A program that reads a 5 KB file a byte at a time does that five thousand times, and the file service reads the same blocks five thousand times. On Unix the same loop is five thousand system calls, which is bad; here it is five thousand conversations between two programs, which is worse.

The cure is fifty years old and has nothing to do with the kernel: **keep some memory in the program, and talk to the system a buffer at a time.**

- **Reading:** one `read` fills the memory. `fgetc` hands out bytes from it. Only when it is empty is there another `read`.
- **Writing:** `fputc` copies a byte into the memory. Only when it is full is there a `write`, of all of it.

That memory, with the descriptor it stands in front of, is a *stream* — C's `FILE`. Everything in `<stdio.h>` that is not `printf` is this idea.

## What a stream is

```c
struct _FILE {
    int descriptor;          /* what read and write are called with       */
    unsigned flags;          /* what it may do, is doing, and has met     */
    unsigned char *buffer;   /* the memory                                */
    size_t size;             /* how much of it there is                   */
    size_t position;
    size_t filled;
    int pushed;              /* the byte ungetc put back, or EOF          */
    unsigned char *own;      /* the buffer it came with (for setvbuf)     */
    size_t own_size;
    unsigned char single;    /* the buffer of a stream that keeps nothing */
    FILE *following;         /* the next stream fopen made                */
};
```

The same buffer is used in two different ways, and a stream is doing one or the other:

```
writing    [ waiting to be sent | free                      ]
            0                  filled                      size

reading    [ handed out | not yet read | nothing            ]
            0        position        filled                size
```

Two flags, `READING` and `WRITING`, say which picture is true. (Neither, just after `fopen` or `fseek`: the buffer is empty and either may start.)

### The price: two opinions about where the file is

A descriptor has an offset (entry 40: a number in the C library's table). A stream sits in front of it, and the moment there is a buffer, **the stream and the descriptor disagree**:

- After `fputc`, the program has written a byte and the file has not got it. The descriptor is *behind* by `filled` bytes.
- After one `fgetc`, the program has read a byte and the descriptor has read 1023. The descriptor is *ahead* by `filled - position` bytes.

Most of `stdio.c` is keeping that straight. Every place where the difference would show, the two are brought together first:

| When | What is done |
|---|---|
| The buffer is full (writing) | `write` it all; `filled = 0` |
| `fflush`, `fclose`, `exit` | the same |
| Reading starts on a stream that was writing | flush first |
| Writing starts on a stream that was reading | `lseek` the descriptor **back** by what was read and not handed out, and forget it |
| `fseek` | flush if writing; if reading and the move is "from here", subtract the read-ahead from the distance; then `lseek` and empty the buffer |
| `ftell` | the descriptor's place, **plus** what is waiting, or **minus** what is read ahead |

`ftell` is my favourite of these because it shows the disagreement as arithmetic:

```c
long ftell(FILE *stream)
{
    off_t place = lseek(stream->descriptor, 0, SEEK_CUR);
    if (place < 0)
        return -1;
    if (stream->flags & WRITING)
        return place + (long)stream->filled;
    return place - (long)unread(stream);
}
```

And the test program shows it from outside. After a single `fgetc` on a 1690-byte file:

```c
check(fgetc(in) == 'l' && ftell(in) == 1, "one fgetc: the stream is at 1");
check(lseek(fileno(in), 0, SEEK_CUR) > 1, "and the descriptor is further on: bytes were read ahead");
```

Both are true at once. The stream is at 1; the descriptor is at 1023.

## Reading

`fgetc` is the whole of reading; the rest is loops around it.

```c
int fgetc(FILE *stream)
{
    if (begin_reading(stream) < 0)
        return EOF;
    if (stream->pushed != EOF) {
        int byte = stream->pushed;
        stream->pushed = EOF;
        return byte;
    }
    if (stream->position == stream->filled && refill(stream) < 0)
        return EOF;
    return stream->buffer[stream->position++];
}
```

Three things in it are worth saying out loud.

**It returns `int`, not `char`.** A byte is 0 to 255. `EOF` is -1. The function must be able to return 257 different things, so it cannot return a `char`. The classic C bug — `char c = getchar(); while (c != EOF)` — either never ends or ends at the first byte 255, depending on whether `char` is signed. I had read about this bug for years; writing the function is what made the reason obvious. The buffer is `unsigned char` for the same reason: byte 200 must come back as 200, not as -56.

**The end is remembered.** When a `read` returns 0, the stream sets `AT_END`, and from then on does not ask again until `clearerr` or a move. `feof` reports that flag. So `feof` is *not* "is the next read going to fail?" — it is "did a read already find the end?". After reading the last byte of a file, `feof` is still false. It becomes true on the read after. The loop `while (!feof(f)) { fgets(...); use(line); }` therefore uses the last line twice, and I now see exactly why.

**`ungetc` has a slot of its own.** C promises one byte can always be put back. I could have put it in the buffer by stepping `position` back — but at the start of the buffer there is nowhere to step. A separate `int pushed` always has room.

`fgets` is a loop of `fgetc` that stops after a new line or when the room runs out. `fread` copies from the buffer, and — when asked for more than a buffer's worth with the buffer empty — reads **straight into the caller's memory**, because copying into the buffer first would be work for nothing.

## Writing, and the three ways text can wait

```c
static int put(FILE *stream, const void *from, size_t count)
{
    const unsigned char *bytes = from;
    size_t left = count;
    while (left > 0) {
        if (stream->filled == 0 && left >= stream->size)
            return send(stream, bytes, left);       /* straight out */
        size_t fits = stream->size - stream->filled;
        if (left < fits)
            fits = left;
        memcpy(stream->buffer + stream->filled, bytes, fits);
        stream->filled += fits;
        bytes += fits;
        left -= fits;
        if (stream->filled == stream->size && flush_waiting(stream) < 0)
            return -1;
    }
    if ((stream->flags & BY_LINE) && memchr(from, '\n', count) != NULL)
        return flush_waiting(stream);
    return 0;
}
```

Every writing function — `fputc`, `fputs`, `fwrite`, `printf` — ends up here.

A buffer raises a question that `write` never did: **how long may text wait?** The answer depends on who is reading.

| Mode | Text is sent | Used for | Why |
|---|---|---|---|
| Fully buffered | when the buffer (4096 bytes) is full | files; `stdout` into a file or pipe | fewest messages; nobody is watching |
| Line buffered | at each new line | `stdout` on the screen | a person is watching, and expects to see a line when it is printed |
| Unbuffered | at once | `stderr` | a complaint still in a buffer when the program crashes is a complaint nobody reads |

Whether `stdout` is the screen is not known until the program runs, so the question is asked the first time it is written to — with a new function, `isatty(1)`, which asks the runtime what the shell said (entry 31's `SCREEN` flag). The name is Unix's: a "tty" was a teletype.

That is the line from the top of this entry:

```c
printf("printf was called first");
write(STDOUT_FILENO, "write was called second - ", 26);
printf("\n");
```

```
write was called second - printf was called first
```

`printf`'s text sat in the buffer waiting for its new line; `write` waited for nothing. Before this entry the two came out in the order they were called. **Mixing `printf` and `write` on one descriptor is now a thing to be careful with**, as it is on every other system.

### "Not buffered" without a special case

An unbuffered stream could have been `if (unbuffered) write(...) else ...` in every function. Instead it has a buffer of **one byte**, inside the stream itself (`single`). A buffer of one is full the moment anything is in it, and the "more than a buffer's worth goes straight out" rule at the top of `put` catches every write of one byte or more. No function below knows that unbuffered streams exist.

### One `fprintf(stderr, …)` is still one write

That one-byte buffer has a consequence. `printf`'s text arrives in pieces — for `"%s: %d\n"`: the string, then `": "`, then the digits, then the new line. On a stream that keeps nothing, that would be four writes, and another program's text could land between them on the screen. So `vfprintf` **lends** such a stream a 256-byte buffer from its own stack for the length of the call, flushes it, and takes it back:

```c
unsigned char lent[256];
unsigned char *its_buffer = stream->buffer;
size_t its_size = stream->size;
int lend = its_size < sizeof lent && stream->filled == 0;
if (lend) {
    stream->buffer = lent;
    stream->size = sizeof lent;
}
int total = __taaj_format(into_stream, &target, format, arguments);
if (lend) {
    if (flush_waiting(stream) < 0)
        target.failed = 1;
    stream->buffer = its_buffer;
    stream->size = its_size;
}
```

### A question before an answer

One more rule, small and old: before `stdin` is read, a line-buffered `stdout` is flushed. A program that prints `name? ` and then reads means the question to be seen before the answer is waited for, though it did not end in a new line.

## The end of the program

Text waiting in a buffer when the program ends must go somewhere. A program that ends by returning from `main` has no other moment to send it.

`exit` is in the Rust half of the library (entry 36). The streams are in the C half. So for the first time **Rust calls C**:

```rust
extern "C" {
    /// Writes out what every stream still holds (libc/c/stdio.c).
    fn __taaj_flush_streams();
}

#[no_mangle]
pub extern "C" fn exit(code: c_int) -> ! {
    // SAFETY: a C function taking and returning nothing, which touches
    // only the library's own streams.
    unsafe { __taaj_flush_streams() };
    taaj::exit(i64::from(code))
}
```

`extern "C" { fn … }` is a *declaration*: it tells Rust "a function with this name and this shape exists somewhere; the linker will find it". Rust cannot check that, which is why calling it needs `unsafe` and a comment saying why it is all right. Until now the arrows only went the other way (C's `fopen` calls Rust's `open`). Now the two halves call each other, and it does not matter, because to the linker both are just names in one archive.

For `exit` to flush every stream it must be able to find them: `fopen` puts each new stream on a linked list (`following`), and `fclose` takes it off.

And `_exit` — which has existed since entry 36 as a twin of `exit` — finally differs from it: **`_exit` flushes nothing.** That is its purpose: for ending a program whose state can no longer be trusted. The test:

```c
printf("kept\n");
fflush(stdout);
printf("this is lost");
_exit(0);
```

run as `cstdio lost > clost` must leave a file holding `kept` and a new line, and nothing else.

A program that is *killed* (Ctrl+C), or that crashes, loses what was in its buffers too. That is the same on every system, and it is the reason `stderr` keeps nothing.

## `fopen`

```c
FILE *stream = malloc(sizeof *stream + BUFSIZ);
if (stream == NULL)
    return NULL;
int descriptor = open(path, how, 0666);
```

Two small decisions:

- **One allocation, not two.** The buffer starts where the stream ends: `stream->own = (unsigned char *)(stream + 1)`. `stream + 1` is pointer arithmetic on a `FILE *` — "the address one whole `FILE` further on" — which is exactly the first byte after the struct. One `malloc`, one `free`, and no way to have a stream without its buffer.
- **Memory first, file second.** `fopen(path, "w")` empties the file. If the `malloc` came after the `open` and failed, the file would have been emptied on the way to an error. Asking for memory first means a failed `fopen` has changed nothing.

The mode string is translated to `open`'s flags from entry 40:

| Mode | `open` flags | Meaning |
|---|---|---|
| `"r"` | `O_RDONLY` | read; the file must be there |
| `"w"` | `O_WRONLY \| O_CREAT \| O_TRUNC` | write; made or emptied |
| `"a"` | `O_WRONLY \| O_CREAT \| O_APPEND` | write at the end; made or kept |
| `+` after any | `O_RDWR` instead | both ways |
| `x` | `\| O_EXCL` | with `w`: fail if it is there |
| `b` | — | allowed, means nothing here |

`stdin`, `stdout` and `stderr` are three `static` streams with `static` buffers, so a program that only prints never calls `malloc`.

## Also here: `rename`, `remove`, `isatty`, `setvbuf`

- **`rename(from, to)`** — in the Rust half, a thin layer over the file service's rename (entry 29). **It replaces nothing:** if `to` exists it fails with `EEXIST`, where Unix would silently replace the file. The file service refuses to make a file vanish as a side effect of moving another, and I did not want to fake the Unix behaviour with "remove, then rename" — two requests, with a moment between them where a crash loses the file.
- **`remove(path)`** — `unlink` under its C name. Files only.
- **`isatty(fd)`** — 1 for the screen. Standard error always is; standard output if the shell said so; standard input never (a program is given a file or a pipe to read, or nothing). New `errno`: `ENOTTY`, 25, "not the screen".
- **`setvbuf`** — chooses among the three modes, with the stream's own buffer or one the program gives. I added it mostly because without it I could not *test* the modes: `stdout` decides once whether it is on a screen, and a test wants both.

## Testing C on the host, with the system swapped out

`stdio.c` asks the rest of the library for seven things — `read`, `write`, `lseek`, `open`, `close`, `unlink`, `isatty` — and `malloc`/`free`. Those are exactly the things the machine I develop on also has. So `stdio.c` can be compiled for the host and tested there, where a test takes a second instead of a boot.

The obstacle: the host already has an `fopen`. Two functions with one name cannot be in one program.

For `printf` (entry 39) I renamed with `-Dsnprintf=taaj_snprintf` on the command line. That does not scale to forty names, and it fights the headers (`#define stdout stdout` and a `-Dstdout=…` cannot both win). The tool for the job turned out to be **`objcopy --redefine-syms`**: compile `stdio.c` *exactly as it is compiled for TheTaaJ*, and then rename symbols **in the object file**:

```
fopen taaj_fopen
fgets taaj_fgets
...
read  taaj_read
write taaj_write
errno taaj_errno
```

The first group are names the object *defines*: they get out of the host's way. The second group are names it *uses*: after renaming, `stdio.o` no longer calls `read`, it calls `taaj_read` — **and that is a function in my test.** The system under the library has been swapped out without changing a line of the library.

The test's replacements pass each call to the real one, and on the way they:

- **count.** "A thousand `fputc` is one `write`" becomes an assertion: `CHECK(writes == 1, …)`.
- **shorten.** A `read` or `write` may do less than asked. Here it can be made to do 10 bytes, or 7, every time — and `fread` must still fill what it was given.
- **fail.** A disk that stops working cannot be arranged in a boot test. Here it is `fail_writes = 1`.

Descriptors 0, 1 and 2 never reach the host: 1 and 2 are kept in memory to be looked at, and 0 reads from a string. That is how "the question was shown before standard input was read" is checked: the fake `read` notes how much had been shown at the moment it was called.

Where the host's stdio does the same thing, both are run through the same script (a macro, expanded once with the host's names and once with mine) on copies of the same file, and every answer must agree — 29 steps of `fseek`/`ftell`/`fread`/`fwrite`/`ungetc`/`rewind`, and `fgets` with four buffer sizes.

Then I broke the library on purpose, nine ways, and ran the test against each:

| Deliberate break | Checks that failed |
|---|---|
| `stdin` does not flush `stdout` | 1 |
| read-ahead not given back (`lseek` removed) | 4 |
| `fseek(SEEK_CUR)` ignores the read-ahead | 2 |
| `ftell` ignores what is waiting | 2 |
| no flush at a new line | 5 |
| `exit` skips the `fopen`ed streams | 2 |
| `stderr` not lent a buffer | 4 |
| the end is not remembered | 2 |
| `isatty`'s `ENOTTY` leaks into `errno` | 1 |

Nine of nine noticed. (After the last entry's "expectation that could not fail", I wanted to know this before trusting 124 green checks.)

## The design, in one table

| Decision | What I chose | What I rejected, and why |
|---|---|---|
| Language | C, on top of `read`/`write` | Rust — possible, but `stdio` needs nothing from the runtime, and in C it can be tested on the host against the host's own |
| Buffer size | 4096 | 1024 (one message) — a message's worth of data is a little *under* 1024 (the header and the file's name take room), so each flush of 1024 would be a full message and a scrap: 8 messages for 4096 bytes, where one flush of 4096 is 5 |
| Stream and buffer | one allocation | two — two failures to handle, and a stream that might have no buffer |
| Unbuffered | a buffer of one byte | a flag checked in every function |
| `stderr` and `printf` | lent a stack buffer per call | truly unbuffered — one `fprintf` becomes several writes, and lines interleave |
| `stdout`'s mode | asked once, at first write (`isatty`) | at start-up — a program that never prints should not ask; fixed — then either the screen stutters or files are slow |
| Changing direction on `r+` | handled: flush, or seek back | requiring `fseek` between, as C permits — a silent corruption for anyone who forgets |
| `ungetc` | its own slot, one byte | stepping back in the buffer — no room at position 0 |
| The end | remembered until `clearerr`/`fseek` | asking again each time — C says the indicator sticks, and programs loop on it |
| A failed flush | the bytes are dropped, `ferror` set | kept — and then every later call fails for ever on the same bytes |
| `fopen` order | `malloc`, then `open` | `open` first — a `"w"` that then fails has already emptied the file |
| Finding streams at `exit` | linked list through the streams | a fixed table — a second limit beside the descriptor table's 32 |
| `rename` onto an existing file | `EEXIST` | remove-then-rename — not atomic; a crash between loses the file |
| Host test | `objcopy --redefine-syms` | `-D` renames (fights the headers); testing only in the booted OS (no fault injection, minutes per run) |

## The bugs

**The test that depended on who ran it.** The host test passed when I ran it by hand and failed inside `make test64`:

```
line 649: not so: neither has a place to move to or ask about
```

The check was that `fseek(stdout, …)` fails. My replacement `lseek` passed descriptor 1 on to the host — and on the host, descriptor 1 is whatever the test's output is. By hand that was a terminal, which cannot be moved about in. Under `make test64` it was a log file, which can. The library was right; the test was asking a question about its own surroundings. The replacement `lseek` now refuses 0, 1 and 2 itself, as the replacement `read` and `write` already kept them away from the host. I only found this because the full suite runs with its output redirected.

**A test that expected the wrong thing.** I wrote "a `printf` longer than the buffer sends a buffer's worth and keeps the rest". It sends *all* of it, at once: with nothing waiting and more than a buffer to go, `put` skips the buffer. The code was doing what I designed and the test what I misremembered. I kept both cases as checks: straight out when nothing waits; a buffer's worth when ten bytes do.

**Arithmetic in my head.** The boot test writes 200 lines, `line 0` to `line 199`, and checks sizes. I wrote 1890. It is 10 × 7 + 90 × 8 + 100 × 9 = 1690. Caught on re-reading, before it ran.

**A number I guessed.** The boot test expects the line `streams: N checks, all passed`. I wrote 51 into the Makefile before the program had ever run; it has 52. The first boot failed on exactly that and nothing else. Harmless, and a reminder that an expected value should be copied from a run I have read, not from a count in my head.

**`fgets` and an old failure.** My first `fgets` returned `NULL` whenever the stream's error flag was set — but the flag stays set until `clearerr`, so one failed read long ago would have made every later `fgets` fail. It now returns `NULL` for a failure only when *this* call's read failed. I found it reading the code, and the host test has the case: after a failing read, "when the disk is back, reading goes on".

## How I checked it

**Automated** (`make test64`):

- **Host, `make libc-test`: 124 checks of `stdio.c`** with the address and undefined-behaviour sanitizers on, covering: write buffering (1000 `fputc` → 0 writes, then 1 at `fclose`; the file empty until then); read buffering (1000 `fgetc` → 1 read); the end (not at the last byte, sticky, cleared by `clearerr`); `fflush` on a stream being read; `fgets` against the host's with rooms of 512, 64, 7 and 2 bytes, with whole reads and reads of 5 bytes; `fread`/`fwrite` of 20 000 bytes in items of 8, 3 and 7, including the short last read; overflow of size × count; reads limited to 10 bytes and writes to 7; a failing disk for writes (noticed at `fflush`, `ferror`, `errno`, the bytes dropped, `fclose` returns `EOF`) and for reads; a 29-step seek script against the host's, and the files compared afterwards; changing direction without `fseek`; `ungetc` five ways; `"a"` and `"a+"`; every bad mode; `"wx"`; writing to a read stream and reading from a write stream by every function; `setvbuf`'s three modes; `stdout` by the line (one write per line, `puts` one write, `errno` untouched); the question-before-answer flush; `stdout` fully buffered and the flush `exit` calls, with a stream closed from the middle of the list; `stderr` (one write per `fprintf`, 700 bytes arriving whole, `perror`); `remove`; `fclose(stdout)`.
- **The nine deliberate breaks** above — run by hand once, not part of `make test64`.
- **In the booted system, BIOS and UEFI:**
  - `with files cstdio check` → `streams: 52 checks, all passed`: the same ground on the real file service — 200 lines written with `fprintf` (the file empty on disk until `fflush`), read back with `fgets`; the stream at 1 while the descriptor is further on; `fseek`/`ftell`/`rewind`; `r+` both ways; `a+`; 10 000 bytes by `fwrite` and back by `fread` in three unequal parts; 29 streams at once and all returned; `rename` (and its refusals: onto an existing file, of a missing one); `remove`; `isatty` three ways; `errno` after the first `printf`.
  - `cstdio lines welcome` (no grant) → `cstdio: welcome: this program was not given the files`.
  - `cstdio lines < welcome` → `   1  Hello from the host`: `fgets` on standard input.
  - `with readonly cstdio lines long > cnum` → the host numbers the 300-line original itself with `awk` and compares, byte for byte.
  - `with files cstdio copy long ccopy` → `ccopy: 4692 bytes`, and the host compares `ccopy` with the original.
  - `cstdio order` → `write was called second - printf was called first`.
  - `cstdio end > cend` → the host checks `cend` holds exactly `no new line, and still here`: `exit` flushed it.
  - `cstdio lost > clost` → the host checks `clost` holds exactly `kept` and a new line: `_exit` did not.
  - The host confirms neither `cstdio.tmp` nor `cstdio.new` is left on the disk.
  - Everything from the earlier entries still passes with buffering underneath — `pout` (entry 39) is still, byte for byte, what `cprint` printed.

**Not tested:**

- A program killed with Ctrl+C while text waits in a buffer (it is lost — by design, but I have not watched it happen).
- The file service stopping under a stream *in the booted system*. The host test covers the library's side of it with a fake; the real `EIO` path from the service is the one entry 40 also left untested.
- A full disk at `fclose`.
- `setvbuf` in the booted system (host only).
- Two streams open on one file, one writing and one reading.
- `stdin` when nothing is given: `getchar()` returns `EOF` at once. A C program still cannot read the keyboard.

## What is still missing

- **`scanf` and its family.** Reading numbers is `fgets` and your own parsing, for now.
- **The keyboard as standard input.** The biggest hole for a "real program": nothing interactive can be written in C yet.
- `rename` does not replace; `remove` does not remove directories; there is still no `mkdir`, `stat`, or `opendir` from C.
- `tmpfile`, `freopen`, `fdopen`, `getline`, `fgetpos`/`fsetpos`, wide characters.
- `atexit`: `exit` flushes the streams and does nothing else.
- `ftell` on a stream opened with `"a"` is only right after the first flush (before it, the descriptor does not yet know where the end is).
- A stream is not safe to use from two threads. A program has one.
- Every `read` is still one round trip of at most 1023 bytes, so a 4096-byte buffer is refilled 1023 bytes at a time. The buffer makes it one round trip per kilobyte instead of one per byte; making it one per four kilobytes means a bigger message or a shared-memory read, which is the file service's business.

## What I learned

- **`stdio` is not about files. It is about not asking.** Every function in it exists to turn many small requests into few large ones. On a microkernel, where a request is a conversation between two programs, it matters more than it ever did on Unix.
- **A buffer creates a second truth.** The stream's position and the descriptor's are different numbers, and the whole craft is knowing every place where the difference could be seen.
- **`fgetc` returns `int` because it has 257 answers.** I knew the rule. I had never seen the reason from the inside.
- **`feof` looks backwards, not forwards.** It says a read *has* hit the end, not that the next one will.
- **Buffering changes what a program does, not just how fast.** `printf` then `write` now come out in the other order. `printf` without a new line, then a crash, prints nothing. These are the two classic "my output is missing" puzzles of C, and I have now built the machine that causes them.
- **Three modes, three readers.** A person wants lines. A file wants blocks. A complaint wants out immediately.
- **A special case can sometimes be made into an ordinary one.** "Unbuffered" as "a buffer of one byte" removed a branch from every function.
- **`objcopy --redefine-syms` swaps the system under a library.** Rename what it defines to get it out of the way; rename what it calls to put the test in the way. No `#ifdef TEST` in the code under test.
- **A test must not ask about its own surroundings.** Mine passed on a terminal and failed into a log file, with the same code.
- **`extern "C" { fn f(); }` in Rust is a promise I make to the compiler**, not one it makes to me — hence `unsafe` at the call.
- **`pointer + 1` moves by one whole thing**, not one byte: `(unsigned char *)(stream + 1)` is the first byte after the struct.
- **Break it on purpose.** 124 passing checks told me less than 9 failing ones did.

---

## Testing note

```
make run64
```

The disk `make run64` uses starts with only the programs on it, so the first lines make a file to read.

**Lines, from a file and from standard input**

| Type | You should see |
|---|---|
| `echo first line > hi` | Nothing |
| `echo second line >> hi` | Nothing |
| `cstdio lines hi` | `cstdio: hi: this program was not given the files`, then `cstdio exited with code 1` |
| `with readonly cstdio lines hi` | `   1  first line` and `   2  second line` |
| `cstdio lines < hi` | The same two numbered lines — read from standard input, no files asked for |
| `ls \| cstdio lines` | The directory listing with every line numbered *(not in the automated test)* |
| `with readonly cstdio lines nofile` | `cstdio: nofile: no such file or directory`, then `cstdio exited with code 1` *(not in the automated test)* |

**Copying**

| Type | You should see |
|---|---|
| `with files cstdio copy cstdio copied` | `copied: ` and a number of bytes (the size `ls` shows for `cstdio`) |
| `ls` | `copied` with the same size as `cstdio` |
| `with readonly cstdio copy hi hi2` | `cstdio: hi2: the files are read-only here`, then `cstdio exited with code 1` *(not in the automated test)* |
| `with files cstdio copy nofile x` | `cstdio: nofile: no such file or directory`, then `cstdio exited with code 1` *(not in the automated test)* |

(The automated test copies the 300-line `long`, which only its own disk has, and compares the copy on the host.)

**What waits, and what does not**

| Type | You should see |
|---|---|
| `cstdio order` | `write was called second - printf was called first` — on one line, in that order |
| `cstdio end` | `no new line, and still here`, and then the prompt: the text had no new line and was sent when the program ended |
| `cstdio end > e` then `cat e` | `no new line, and still here` |
| `cstdio lost` | `kept`, and nothing more |
| `cstdio lost > l` then `cat l` | `kept` only: `this is lost` was in the buffer when `_exit` ended the program |

**The checks**

| Type | You should see |
|---|---|
| `with files cstdio check` | `streams: 52 checks, all passed` |
| `ls` afterwards | No `cstdio.tmp` and no `cstdio.new` |
| `with readonly cstdio check` | `cstdio: check needs to make a file (with files cstdio check): the files are read-only here`, then `cstdio exited with code 2` *(not in the automated test)* |
| `cstdio` | `usage: cstdio lines [FILE] \| copy FROM TO \| order \| end \| lost \| check`, then `cstdio exited with code 2` |

**The earlier C programs, now buffered underneath** *(all in the automated test)*

| Type | You should see |
|---|---|
| `cprint one two` | The same seven lines as before, ending `fputs and putchar`, then `cprint: 9 lines went to standard output` |
| `with files cfile check` | `files: 48 checks, all passed` |
| `cmem` | `strings: 28 checks, all passed` and `memory: 15 checks, all passed` |

**On the host**

```
make libc-test
```

now runs four things: 10 string tests, 4 `errno` tests, `format: 123 checks, all the same as the host's printf`, and `stdio: 124 checks, all passed`.

**A program of your own** *(not tested: please try it)* — on the host:

```
cat > libc/programs/mine.c <<'EOF'
#include <errno.h>
#include <stdio.h>
#include <string.h>
int main(int argc, char **argv)
{
    if (argc != 2) {
        fprintf(stderr, "usage: mine FILE\n");
        return 2;
    }
    FILE *file = fopen(argv[1], "r");
    if (file == NULL) {
        fprintf(stderr, "mine: %s: %s\n", argv[1], strerror(errno));
        return 1;
    }
    int byte, lines = 0, bytes = 0;
    while ((byte = fgetc(file)) != EOF) {
        bytes++;
        if (byte == '\n')
            lines++;
    }
    fclose(file);
    printf("%s: %d lines, %d bytes\n", argv[1], lines, bytes);
    return 0;
}
EOF
make cprogs64 C_PROGRAMS=mine
tools/tfstool-rs/target/release/tfstool put build/taaj-disk.img mine build/c/mine
```

(with QEMU closed), then `make run64`:

| Type | You should see |
|---|---|
| `with readonly mine hi` | `hi: 2 lines, 23 bytes` |
| `wc hi` | `hi: 2 lines, 4 words, 23 bytes` — the same lines and bytes |
| `with readonly mine cstdio` | `cstdio: ` some lines and the size `ls` shows — read a byte at a time, and back at the prompt at once |

Delete `libc/programs/mine.c` afterwards, and `rm mine` in the OS.

**Cleaning up**, in the OS: `rm hi copied e l`.

Automated: `make test64`.
