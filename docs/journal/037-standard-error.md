# Standard Error: A Second Way Out

*Journal entry #37 · a program's complaints stop going wherever its output goes — into the file, down the pipe — and reach the screen instead; descriptor 2 for C, `eprintln!` for Rust*

Since redirection (entry 31) there has been a line in my list of known problems that I wrote down and walked past five times:

```
taaj:/> cat none > err
taaj:/> cat err
cat: none: no such file or directory
```

The complaint went into the file. Nobody was told anything went wrong, and the file — which was meant to hold what `cat` produced — holds an English sentence about a different file. With a pipe it is worse: `cat none | wc` answers `1 lines, 7 words, 37 bytes`, counting the words of the error message as if they were data.

After this entry:

```
taaj:/> cat none > err
cat: none: no such file or directory
cat exited with code 1
taaj:/> cat err
taaj:/> cat nofile | wc
cat: nofile: no such file or directory
0 lines, 0 words, 0 bytes
cat exited with code 1
```

(The `exited with code 1` lines are the shell's own, as they always were: the shell never wrote to anything but its screen.)

This is step 2 of the seven towards a C library: (1) toolchain and crt0 — entry 36; (2) **standard error — this entry**; (3) memory and strings; (4) `printf`; (5) file descriptors; (6) `stdio` on files; (7) a real program and the recipe.

It sits in the libc plan because C names the thing — descriptor 2, `stderr` — and a C program written anywhere else already uses it. But most of the work was not in the libc at all.

---

## Why one output is not enough

A program says two different kinds of thing:

- **what it was asked to produce** — the listing, the file's contents, the count;
- **things about itself** — that a file is missing, that it was called wrongly, that it has crashed.

While everything goes to the screen, nobody can tell the difference and nobody needs to. The moment the output can be *sent somewhere* — a file, another program — the two kinds have different readers. The listing is for the file. The complaint is for me, sitting at the keyboard, right now.

Unix solved this in the 1970s in the plainest way possible: give every program **two** outputs, and let whoever starts it point them at different places. That is the whole idea. Nothing in the kernel knows which is which; "standard error" is only a convention about what a program writes where.

## What a stream is here, and what was added

A reminder of how a program gets its output on this system, because standard error is the same thing once more. When the shell asks init to start a program it hands down **handles** — the program's ends of channels — and a small number, the *startup flags*, saying which ones it handed:

```
bit 0  STDOUT      a channel to send output on
bit 1  ARGUMENTS   a channel holding the arguments
bit 2  SERVICES    the program's connection to the registry
bit 3  DIRECTORY   a channel holding the working directory
bit 4  STDIN       a channel to receive input from
bit 5  STDERR      a channel to send complaints on          <- new
bit 7  SCREEN      not a handle: "standard output is the screen"
```

The handles arrive in the order of the bits, with none for a bit that is clear. So the runtime finds one by counting: *the handle for flag F is at position (number of set bits below F)*.

```rust
fn flagged_handle(flag: u64) -> Option<Handle> {
    let flags = STARTUP_ARGUMENT.load(Ordering::Relaxed);
    (flags & flag != 0)
        .then(|| startup_handle(u64::from((flags & (flag - 1)).count_ones())))
        .flatten()
}
```

`flag - 1` is a mask of every bit below the flag (for `1 << 5` it is `0b11111`), so `flags & (flag - 1)` keeps the lower flags that are set and `count_ones()` counts them. I wrote this function for standard input; it needed no change at all for a sixth stream. The new constant was enough:

```rust
/// Standard error: a channel to send on, as standard output is - for
/// what the program has to say that is not its result ...
pub const STDERR: u64 = 1 << 5;
```

`SCREEN` was deliberately put at bit 7, above the handle flags, when I added it — so there was a free bit 5 waiting and nothing had to be renumbered.

## The four places it had to go

```
   shell ──(Run: flags, handles)──► init ──(startup handles)──► program
     │                               │                             │
  1. offers STDERR               2. accepts the flag,          3. write_error()
     = the screen                   passes it on, cut             eprintln!
                                    down to SEND               4. libc: write(2, ...)
```

**1. The shell offers it.** The shell already builds a list of what it hands a program. One more entry:

```rust
let offered = [
    (abi::startup::STDOUT, stdout),
    (abi::startup::SERVICES, services),
    (abi::startup::STDIN, stdin),
    // Standard error is the screen, wherever the output goes: a
    // complaint is for whoever is watching, not for the file or the
    // next program.
    (abi::startup::STDERR, Self::delegate(self.screen)),
];
```

`stdout` is the screen, or a file's writer, or one end of a pipe, depending on the line typed. Standard error does not depend on the line: it is a copy of the shell's own handle on the display, always. (`delegate` duplicates a handle so the copy can be given away and the shell keeps its own.)

**2. init lets it through.** init refuses any request with a flag it does not know — a request it does not understand is not one to guess at — so the new flag had to be added to the mask it accepts, and to the list it walks when it lays out the new program's handles. Each handle is cut down to the one right it is for: standard error, like standard output, may only be *sent on*.

**3. The runtime writes to it.**

```rust
pub fn write_error(bytes: &[u8]) -> Result<u64, Error> {
    if let Some(screen) = flagged_handle(abi::startup::STDERR) {
        write_to_display(screen, bytes);
    }
    debug_write(bytes)
}
```

It is shorter than `write`, which has to ask "is my output the screen, or a stream of bytes for someone else?" and speak two different protocols. Standard error is, for now, always the display, so it always speaks the display's protocol. A program started with no standard error at all — a service started by init — still gets its words into the kernel's debug log.

On top of that, the two macros a Rust programmer expects, built exactly as `print!` and `println!` are:

```rust
#[macro_export]
macro_rules! eprint {
    ($($arg:tt)*) => { $crate::eprint_args(format_args!($($arg)*)) };
}

#[macro_export]
macro_rules! eprintln {
    () => { $crate::eprint!("\n") };
    ($($arg:tt)*) => { $crate::eprint!("{}\n", format_args!($($arg)*)) };
}
```

A note for myself on reading these, since macros still look like line noise to me. `$($arg:tt)*` means "any number of tokens, call them `arg`" — the macro does not try to understand what was written between the brackets, it passes all of it to `format_args!`, which is the compiler's own machinery for `{}` holes. `$crate` is "the crate this macro was defined in", so the macro works from any program without that program importing `eprint_args`. And `#[macro_export]` puts the macro at the root of the crate, which is why programs write `use taaj::eprintln;`.

The panic handler uses it too: `panic: …` is the clearest case there is of something that is not the program's output.

**4. The libc sends descriptor 2 there.** Until now `write(2, …)` quietly went to standard output. One line changed:

```rust
let written = if fd == 2 { taaj::write_error(bytes) } else { taaj::write(bytes) };
```

This is the "Rust core, C on top" decision paying for itself for the first time: the C library did not grow a second implementation of anything. It got standard error by the runtime getting it.

## Then every program had to be read

A second output is worth nothing if programs do not use it. So I went through `ls`, `cat`, `wc`, `rm`, `mkdir`, `mv`, `cp` and the code they share, and sorted every line they print into one of the two kinds:

| Program | Lines moved to standard error | Lines left on standard output |
|---|---|---|
| `ls` | 3 | 6 |
| `cat` | 3 | 1 |
| `wc` | 3 | 2 |
| `rm` | 3 | 0 |
| `mkdir` | 3 | 0 |
| `mv` | 3 | 0 |
| `cp` | 10 | 1 |
| shared file code | 3 | 1 |

The rule I used: *would this line make sense inside the file, if the output had been sent to one?* `usage: cat FILE...` would not. `ls: nodir: no such file or directory` would not. `3 lines, 7 words, 30 bytes` would.

`rm`, `mkdir` and `mv` now have nothing left on standard output: programs that do a thing and say nothing unless it fails. That is how they should look.

## What `chello` shows now

The C test program got one more mode:

```c
if (argc > 1 && same(argv[1], "err")) {
    /* Descriptor 2: not part of the output. With `chello err > f`
     * this line is on the screen, and not in f. */
    const char *complaint = "chello: this line is on standard error\n";
    write(STDERR_FILENO, complaint, length(complaint));
}
```

```
taaj:/> chello err > cerr
chello: this line is on standard error
taaj:/> cat cerr
Hello from C
argc 2
argv[1] err
```

## The design, in one table

| Decision | What I chose | What I rejected, and why |
|---|---|---|
| What standard error *is* | A second channel handed down at start, found by a startup flag | A message type on the output channel ("this chunk is an error") — then every reader of every stream, the file writer and `wc` alike, would have to know about it and filter. Two channels need no reader to know anything |
| Where it points | Always the screen | Redirecting it (`2>`, `2>&1`) — wanted, but it is shell syntax and a separate piece of work; first the stream has to exist |
| When there is none | The words go to the debug log only | Falling back to standard output — that is exactly the bug being fixed, hidden until the day it matters |
| Rights on the handle | Send only, as standard output | The shell's full rights on the display — a program has no business doing to the display what the shell may |
| The shell's own complaints (`nosuch: not found`) | Unchanged: the shell writes straight to its screen | Nothing to do — the shell's messages never went into the file in the first place |
| Which lines moved | Complaints, usage lines, panics | Moving everything a failing program prints — `wc`'s count of the files it *could* read is still its output |
| The C side | `write(2, …)` → `taaj::write_error` | A separate C implementation — there is no reason for two |

## The bugs

Only one worth a line, and it was mine before the code ran: I wrote `eprint_args` calling a type, `ErrorConsole`, that I had not written yet. The compiler said so. It is the twin of the `Console` type that `print!` falls back on when a line is too long for the 512-byte buffer: something that implements `core::fmt::Write`, so that `write_fmt` can push the text through it in pieces.

And one thing that is not a bug but which I checked because it could have been: the order of lines. A program that prints to the screen on standard output and then complains on standard error is sending on two handles — could the complaint overtake the output? No: both handles are the *same channel* to the display, and a channel delivers in the order things were sent.

## How I checked it

**Automated** (`make test64`, on both the BIOS and the UEFI boot):

- `cat none > err` — the host reads the disk afterwards and `err` must be an **empty** file. Before this entry the same test demanded that the complaint be *in* it; I turned the check round.
- `ls nodir > lsout` — the same, for `ls`: `lsout` is empty, and the log holds `ls: nodir: no such file or directory`.
- `cat nofile | wc` — the log holds the complaint, and `wc` says `0 lines, 0 words, 0 bytes`.
- `chello err > cerr` — the log holds `chello: this line is on standard error`, and the host reads `cerr` and finds exactly the three lines of standard output.
- Everything that passed before still passes, which matters here more than usual: every program is now started with one more handle.

**By hand:** the automated checks read the kernel's log, and `write_error` writes to the log whether or not the screen got anything — so a passing test did not prove the words reach the *screen*. I booted once more, typed `cat none > e1`, `chello err > e2`, `cat nofile | wc` and `cat e1 e2`, and read QEMU's screenshot: both complaints are on the screen under their commands, `wc` says `0 lines, 0 words, 0 bytes`, and `cat e1 e2` prints only the C program's three lines.

**Not tested:**

- Each converted line one by one. I tested `cat` and `ls`; the other programs' complaints were converted by the same edit and compile, but I did not run `cp` or `mv` wrongly into a file to see.
- A panic going to standard error.
- A complaint from a background job, or from the middle of a three-program pipeline.
- A display that has stopped while a program complains (the same give-up-after-a-second code as for output, but not exercised through this path).

## What is still missing

- **No `2>`**: standard error cannot be sent to a file, or joined to standard output (`2>&1`). It is always the screen.
- **The shell does not know which is which on the screen**: complaints are not coloured or marked.
- `argv[0]` is still the placeholder `program`.
- C has no `stderr`, no `fprintf`, no `perror` — only `write(2, …)`. Those come with steps 4 and 6.
- The shell still keeps its lists of which programs look and which change files (said in entry 35; still true).

## What I learned

- **Standard error is not a kernel feature.** I half expected to add something to the kernel. I added a constant. It is a convention between a parent and a child about one more handle — and that is what it is on Unix too, where descriptor 2 is only "the third thing that was open when you started".
- **A design is tested by the next feature.** Counting set bits to find a startup handle, and putting `SCREEN` out of the way at bit 7, were choices from two entries ago that I could not have defended very well at the time. This entry is their defence: a new stream cost one constant and three list entries.
- **Two channels beat one channel with labels.** The tempting design was to tag error text inside the output stream. It would have pushed knowledge of errors into everything that reads a stream. Keeping the streams dumb — bytes, and an end — and having *more of them* keeps every reader simple.
- **`$($arg:tt)*`** is "whatever was typed, passed along untouched" — the reason `eprintln!` can be four lines long and still accept everything `println!` does.
- **The boring half is the real half.** The mechanism was thirty lines. Reading eight files and deciding, line by line, *who is this sentence for?* was the part that changed what the system does.

---

## Testing note

```
make run64
```

**Complaints stay on the screen**

| Type | You should see |
|---|---|
| `cat none > err` | `cat: none: no such file or directory` — on the screen, though the output went to a file — then `cat exited with code 1` |
| `cat err` | Nothing: the file is empty |
| `ls nodir > lsout` | `ls: nodir: no such file or directory`, and the line saying `ls` exited with a code |
| `cat lsout` | Nothing |
| `cat nofile \| wc` | `cat: nofile: no such file or directory`, then `0 lines, 0 words, 0 bytes`, then `cat exited with code 1` |
| `cat` *(no file named, no input)* `> u` | `usage: cat FILE...` on the screen *(not in the automated test)* |

**Output still goes where it is sent**

| Type | You should see |
|---|---|
| `ls > list` | Nothing |
| `cat list` | The listing |
| `ls \| wc` | A count, and nothing else |

**From C**

| Type | You should see |
|---|---|
| `chello err` | `Hello from C`, `argc 2`, `argv[1] err`, `chello: this line is on standard error` |
| `chello err > cerr` | Only `chello: this line is on standard error` |
| `cat cerr` | `Hello from C`, `argc 2`, `argv[1] err` — and not the fourth line *(the `cat` is not automated; the host reads the file)* |
| `chello err \| wc` | The standard error line, then `3 lines, 7 words, 32 bytes` *(not in the automated test)* |

**Not tested: please try these**

| Type | You should see |
|---|---|
| `cp > x` | `cp`'s usage line on the screen; `cat x` shows nothing |
| `mv a > x` | The same for `mv` |
| `cat none > x &` | `[1] cat`, the complaint, and at the next prompt a line saying job 1 ended |

**From the host** — with QEMU closed:

```
tools/tfstool-rs/target/release/tfstool ls build/taaj-disk.img
tools/tfstool-rs/target/release/tfstool get build/taaj-disk.img cerr
```

`err` and `lsout` are listed with size 0, and `cerr` is the three lines.

**Cleaning up**, in the OS: `rm err lsout cerr list x u`.

Automated: `make test64`.
