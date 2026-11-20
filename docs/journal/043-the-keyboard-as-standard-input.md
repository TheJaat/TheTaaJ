# The Keyboard as Standard Input: Whose Key Is It?

*Journal entry #43 · a program can now be typed to — a line at a time, with Backspace, Ctrl+D for "no more" — and the question that decided the whole design: when a key is pressed while a program is running, does it belong to the program or to the shell?*

```
taaj:/> cask
your name? ada
hello, ada (typed)
numbers to add, one a line; Ctrl+D ends
40
x
cask: not a number: x
2
total: 42
taaj:/> wc
two words
1 line, 2 words, 10 bytes
```

Until today a program's standard input was a file (`wc < notes`), another program (`cat notes | wc`), or nothing. A program run by its name alone had no input: `read(0, …)` said "the end" at once. Entry 42 called this the largest gap left in the C library, and it was not really a gap in the library. Nothing interactive could be written in *any* language.

This entry is outside the seven libc steps (36–42) and before SMP: I chose to do it first.

---

## The obvious design, and what is wrong with it

The shell already has the keyboard. While a program runs in the foreground the shell is still the one receiving keys — that is how Ctrl+C works (entry 17). And the shell already knows how to feed a program's standard input: for `wc < notes` it makes a channel, gives the program one end, and sends the file down the other (entry 31).

So the obvious design is three lines: *while a program runs, send every line typed down its input channel.*

It is wrong, and the way it is wrong is instructive.

Type `primes` and, while it is still printing, type `ls` and Enter — typing ahead. `primes` never reads its input. With the obvious design, `ls` goes down a channel nobody is reading, `primes` ends, the channel is closed, and the line is gone. The shell never sees it.

Today that line is kept and run when `primes` ends, and my automated test depends on it completely: it types a couple of hundred commands as fast as QEMU accepts keys, and most are typed while the one before is still running. The obvious design would have made the test fail in a different place every run.

The real question is: **when a key is pressed, whose is it?** And the answer is not "the program's, because it is running". It is:

> **A key belongs to whoever reads next.**

If the program reads, the key was for the program. If the program ends without reading, the key was for the shell. Nobody can know which at the moment the key is pressed.

## How Unix answers it

On Unix neither the shell nor the program holds the keyboard. The kernel does, in a thing called the *terminal driver* (the "tty"). Keys go into the terminal's queue. Whoever calls `read` on the terminal next gets them. While a program runs, the shell is not reading — it is waiting for the program to end — so the program gets them if it asks. If it never asks, they are still in the queue when the shell reads again.

The terminal driver also does the line editing: it shows each key, handles Backspace, and hands over nothing until Enter. A program calling `read(0, …)` on a terminal gets a whole line, already corrected. This is called *cooked* (or canonical) mode. It is why the simplest C program can ask a question and have Backspace work without containing a line of code for it.

So "whoever reads next" works on Unix because reading is *pull*: the reader asks, and the queue answers.

## The same answer, without a terminal in the kernel

My kernel has no terminal driver and should not get one: it is a microkernel, and line editing is not its business. The shell holds the keyboard. A channel is *push*: the sender decides when to send.

To get "whoever reads next" over a channel, the reader has to say when it is reading. So there is one new message, going the opposite way down the input channel:

```
program                                shell
   |                                     |
   |   read(0, ...) and nothing held     |
   |------------- [WANT] --------------->|   "I am about to wait for input"
   |                                     |   keys typed now - and any typed
   |                                     |   before and not yet used - are
   |                                     |   this program's: edit a line,
   |                                     |   show it, wait for Enter
   |<-------- [DATA] "ada\n" ------------|
   |                                     |   one asking, one line: keys go
   |                                     |   back to waiting for whoever
   |                                     |   reads next
```

A channel has two ends and each can send and receive, so no second channel is needed. The program's end used to be receive-only; for a keyboard input, `init` now passes it with the right to send as well.

The program has to know its input is this kind, because a file-fed input must *not* be sent `WANT`s (nobody is there to read them; they would pile up in the channel). That is a new startup flag:

```rust
/// Not a handle: standard input is the keyboard - a person, typing.
/// The program gets a channel for its input as always (STDIN), but
/// nothing is sent down it until the program asks ...
pub const KEYBOARD: u64 = 1 << 6;
```

It sits beside `SCREEN` (entry 31), which says the same kind of thing about the output. Neither is a handle; both describe one.

### In the runtime: six lines

Every program — Rust or C — reads standard input through `taaj::Input::read`. The change there is small:

```rust
// A keyboard is asked before it is waited for. If the
// asking cannot be sent, nobody is there to answer.
if self.keyboard && !self.asked {
    if send(channel, &[abi::stream::WANT], &[]).is_err() {
        self.channel = None;
        return 0;
    }
    self.asked = true;
}
```

`asked` is there so that **one asking gets one line**. `read` loops (a message that is not data is skipped), and a second `WANT` for the same wait would leave the shell holding the *next* line for a program that is no longer waiting — the typed-ahead bug again, by another door.

Because it is in the runtime, nothing above it changed. C's `read(0, …)`, `fgets`, `getchar`; Rust's `cat` and `wc` — all of them now read the keyboard, without one of them knowing.

### In the shell: the same loop, one more thing to watch

The shell has one place of waiting, `attend` (entry 33), which watches the keyboard, every running program, and every redirected output. A program's keyboard input is one more thing in it:

- The job's input is a `Feed` as before, with a new kind of supply: `Supply::Keyboard`.
- `attend` watches the shell's end of that channel. A `WANT` arriving sets `feed.wanted`.
- A key that arrives while a foreground program runs goes into `typed_ahead`, exactly as before. **That did not change at all.**
- At the top of the loop, if the program `wanted` input, keys are taken from `typed_ahead` and typed into a line for it:

```rust
fn type_to(&self, feed: &mut Feed) -> bool {
    while feed.wanted {
        let Some(key) = self.typed_ahead.borrow_mut().pop_front() else { break };
        let mut typing = self.typing.borrow_mut();
        let end: &[u8] = match key.as_slice() {
            b"\n" => {
                self.tell(b"\n");
                b"\n"
            }
            CTRL_D if typing.bytes().is_empty() => return false,
            CTRL_D => b"",
            key => {
                typing.key(key, |shown| self.tell(shown));
                continue;
            }
        };
        // ... send the line and `end` as one DATA message ...
        typing.clear();
        feed.wanted = false;
    }
    true
}
```

So there is a single queue of keys, and two readers who take from it: the shell's prompt, and a program that has asked. That is the terminal's queue, in the shell.

The line editing — add a character, Backspace over a whole UTF-8 character, ignore control keys — used to be written inline in the shell's main loop. It is now a small `Typing` struct used for both the command line and a line typed to a program, so the same keys do the same things in both.

### Ctrl+D, and Ctrl+C

**Ctrl+D** means "no more input". At the start of a line the shell closes its end of the channel — which is how *any* input ends (entry 31): the program's `read` returns 0, `fgets` returns `NULL`. In the middle of a line it sends what has been typed with no new line after it, as Unix does.

It is worth being clear that Ctrl+D is not a character the program receives. There is no "end of file byte". The end of an input is the absence of anything further, and Ctrl+D is the user's way of telling the shell to arrange that.

**Ctrl+C** stops the foreground program, as before — and now also throws away what was typed and not yet read. Those keys were meant for the program being stopped. Without this, `cat`, then `abc`, then Ctrl+C could leave `abc` at the start of the shell's next line. (Unix does the same: an interrupt flushes the terminal's queue.)

### Who gets the keyboard

- A program in the foreground with no `<` and nothing piped into it: the keyboard.
- In a pipeline, only the first program: `cask | wc` types to `cask`.
- A program in the background (`&`): no input at all, as before. What is typed is for the shell and for the program in front.

## What changed for programs that already existed

`cat` and `wc` with no arguments used to print their usage line and exit with 2, because they had no input. They now have one, and read it:

```
taaj:/> cat > notes
this is typed
and so is this
taaj:/> cat notes         (after Ctrl+D)
this is typed
and so is this
```

`cat > file` is suddenly the system's first way to write a file of several lines. It is not an editor, but it is more than `echo`.

In C, `isatty(0)` is now true when the input is typed, and the stdio rule from entry 41 — reading `stdin` first flushes a line-buffered `stdout` — finally has a use: `printf("your name? ")` with no new line is on the screen before the program waits for the answer.

## The design, in one table

| Decision | What I chose | What I rejected, and why |
|---|---|---|
| Whose is a key typed while a program runs? | Whoever reads next | The running program's — typing ahead breaks, and a program that never reads swallows the user's next command |
| How the shell knows a program is reading | The program says so: `WANT`, up its own input channel | Guessing (it cannot be known); a kernel call "is anyone blocked on this channel?" (new kernel surface for one user) |
| Where the terminal lives | In the shell | In the kernel (Unix's answer; not a microkernel's job); a separate terminal service (right in the long run — see below — but a new service and a protocol for something the shell already does) |
| Channel for `WANT` | The input channel itself, in the other direction | A second channel — one more handle at startup for one byte |
| How a program knows to ask | A startup flag, `KEYBOARD` | Always asking — a file's or pipe's sender would be sent messages it never reads |
| What a program receives | Whole lines, edited by the shell | Raw keys — every program would need its own Backspace |
| How many lines per asking | One | All typed so far — the lines after the first may be for the shell |
| Ctrl+D | Close the channel (start of line); send without new line (mid-line) | A special byte in the data — then every reader must know it, and files could contain it |
| Ctrl+C | Also discards unread typed keys | Keeping them — they land on the shell's next line |
| Background programs | No input | The keyboard — two readers of one keyboard, and no way to say which was meant |

## The bugs

**A test that would have hung, found by reading before running.** The automated test has long typed `run cat`, expecting `usage: cat FILE` and exit code 2. With the keyboard as its input, `cat` with no arguments is no longer a mistake — it waits for typing. The test would have sat there, and `cat` would have echoed every later command of the test back as text. I looked through the test's key list for programs run with no arguments before running it, found this one, and changed the test to type a line and Ctrl+D after it and expect exit code 0.

**The typed-ahead race in my own new test.** I wrote the Ctrl+C test as: `cat`, then `abc`, then Ctrl+C, then `echo clean` — expecting the shell's next line to be exactly `echo clean`. Thinking it through before running: the test types fast, so `abc` might arrive *before* `cat` has asked for input, sit in the typed-ahead queue, survive the Ctrl+C, and come out as `abcecho clean`. That was not a flaw in the test. It was a real bug the test had found on paper, and it is why Ctrl+C now empties the queue.

**A check that had become false.** `cstdio check` (entry 41) asserted that standard input is *not* the screen's partner: `isatty(0) == 0`. The first full run failed on exactly that line — `cstdio: failed: standard input is not` — because it now is. The library was right and the old check was describing the old world. It now accepts either answer, as long as a "no" comes with its reason (`ENOTTY`).

**`strcspn` was not there.** The interactive test program cuts the new line off a line with `text[strcspn(text, "\n")] = '\0'`, the usual idiom, and the linker had never heard of `strcspn`. Entry 42's lesson again: the library's gaps are found by writing programs. `strspn` and `strcspn` are now in `string.rs`, with a host test.

**Too many arguments.** Adding `keyboard: bool` to the shell's `start` function made it eight parameters, and Clippy refuses more than seven. The two that describe the input became one small struct, `Stdin { channel, keyboard }` — which is what they were all along.

## How I checked it

**Automated** (`make test64`; both the BIOS and UEFI boots):

- `run cat`, then `echoed`, then Ctrl+D → `cat exited with code 0`. (Before: the usage line and code 2.)
- `wc`, then `two words`, then Ctrl+D → `1 line, 2 words, 10 bytes`.
- `cat > typed`, then `kept line`, then Ctrl+D → the host reads the file `typed` off the disk: `kept line`.
- `cat`, then `abc`, then Ctrl+C, then `echo clean` → `cat was stopped`, and the shell's log shows its next line as exactly `echo clean`.
- `cask`, then `ada`, `40`, `x`, `2`, Ctrl+D → `hello, ada (typed)`, `cask: not a number: x`, `total: 42`. This is C's `fgets` on `stdin`, `isatty(0)`, and the prompt printed without a new line.
- `cask one`, then `first`, then `echo after` → `one line: first (typed)` and then `after`: the program asked once and got one line; the second line typed went to the shell, and ran.
- `echo ada > who`, `cask one < who` → `one line: ada (from a file or another program)`: an input that is not the keyboard is not asked, and still works.
- **Everything else in the test**, which is the real check of "whoever reads next": a couple of hundred commands typed without waiting, most of them while the previous program was still running and holding a keyboard input it never asked for. Had any program swallowed a key, a later command would have come out wrong.
- Host: one new test of `strspn`/`strcspn` (11 string tests now).

**Not tested:**

- Ctrl+D in the middle of a line (sends the part typed, no new line).
- Backspace while typing to a program. It is the same `Typing` code as the shell's own line, which the test does exercise with Backspace.
- A line longer than 256 bytes typed to a program (further keys are ignored until Enter, as at the prompt).
- A pipeline whose first program reads the keyboard (`cask | wc`).
- A program that ends while a half-typed line is on the screen (the line is discarded; the characters stay on the screen).
- A program that closes its standard input and goes on running.
- Two programs wanting the keyboard at once cannot happen — only the foreground job is given it — but nothing tests that a background job really gets none beyond what entry 33 already tested.
- The `run`-only programs built into the kernel image now also receive a keyboard input; none of them reads it.

## What is still missing

- **Single keys.** A program gets lines, never keys: no arrow keys, no "press any key", no turning the echo off for a password. Unix calls the alternative *raw mode* (`termios`). An editor or a game needs it. It would be a second kind of asking: "give me keys, not lines".
- **The terminal is inside the shell.** It works because there is one shell and one screen. With two shells — or a program that wants to run another program and lend it the keyboard — the terminal should be its own service that the shell and programs both talk to. That is what Unix's tty is, and where this is heading; today's protocol (`WANT`, lines back) is what such a service would speak.
- **No history, no arrow keys, no tab completion** at the prompt or anywhere else.
- **`scanf`.**
- A program cannot tell "the user pressed Ctrl+D" from "the input was a file and it ended". Nor can a Unix program.
- After Ctrl+D the input stays ended for that program. (On Unix a program can go on reading a terminal after an end-of-file; almost none do.)

## What I learned

- **"Whose input is this?" is decided by reading, not by running.** A key belongs to whoever reads next, and nobody knows who that is when the key is pressed. Every design that guesses is wrong some of the time.
- **Pull and push are different protocols, not different speeds.** A file can be pushed at a reader. A person's typing has to be pulled, because it has more than one possible reader.
- **Push becomes pull with one message going the other way.** `WANT` is a credit: permission to send exactly one line.
- **Cooked mode is a service to programs.** Line editing lives *outside* the program on Unix, which is why `fgets` gives a corrected line. I had used that all my life without knowing where it happened.
- **Ctrl+D is not a character.** It is an instruction to whoever holds the keyboard to end the input. The end of a stream is always "nothing more came", never a value in the stream.
- **A channel with two ends is already two-way.** I had been using a program's input and output channels in a single direction each; the reverse direction was there for the taking.
- **Putting a change in the lowest shared layer makes it free above.** Six lines in `Input::read`, and C's stdio, `cat` and `wc` all gained the keyboard without being touched.
- **My test suite was a specification I had not written down.** "Typing ahead works" was never a stated requirement; the fast-typed commands of the boot test had been enforcing it all along, and they ruled out the obvious design before I wrote a line of it.
- **Think a test through before running it.** Both bugs in this entry were found by asking "what happens if these keys arrive early?" — one in an old test, one in the new code.

---

## Testing note

```
make run64
```

**Typing to a program**

| Type | You should see |
|---|---|
| `cask` | `your name? ` and the cursor waiting on the same line |
| `ada` then Enter | `hello, ada (typed)`, then `numbers to add, one a line; Ctrl+D ends` |
| `40` Enter, `x` Enter, `2` Enter | After `x`: `cask: not a number: x`. Nothing after the others |
| Ctrl+D | `total: 42`, and the prompt |
| `cask` then `adx`, Backspace, `a`, Enter | `hello, ada (typed)` — the program never saw the `x` *(not in the automated test)* |
| Ctrl+D | `total: 0` |

**Old programs, new input**

| Type | You should see |
|---|---|
| `wc`, then `two words` Enter, then Ctrl+D | `1 line, 2 words, 10 bytes` |
| `cat`, then `echo me` Enter | `echo me` again on the next line: `cat` printed what it was sent. (It was not run as a command.) |
| Ctrl+D | The prompt |
| `cat > typed`, then `kept line` Enter, `and another` Enter, Ctrl+D | Nothing but what you type |
| `cat typed` | `kept line` and `and another` *(the automated test types one line, and reads the file on the host)* |
| `wc`, then `half` and Ctrl+D (no Enter) | Nothing yet: the four bytes were sent without a new line. Ctrl+D again: `0 lines, 1 word, 4 bytes` *(not tested)* |

**Whose key is it?**

| Type | You should see |
|---|---|
| `cask one`, then `first` Enter, then `echo after` Enter | `one line: first (typed)`, then `after`: the program asked for one line and got one; the next went to the shell |
| `primes`, and at once, without waiting, `echo typed ahead` Enter | The primes, and then `typed ahead`: `primes` reads nothing, so the line was the shell's *(the automated test does this with every command it types)* |
| `cat`, then `abc` (no Enter), then Ctrl+C | `^C`, `cat was stopped`, and an empty prompt — `abc` is not on it |
| `echo clean` | `clean` |
| `forever &`, then `cask one`, then `mine` Enter | `one line: mine (typed)`: the background job gets no keys. Then `kill 1` *(not tested)* |

**Not typed**

| Type | You should see |
|---|---|
| `echo ada > who` | Nothing |
| `cask one < who` | `one line: ada (from a file or another program)` |
| `echo grace \| cask one` | `one line: grace (from a file or another program)` *(not in the automated test)* |
| `cask one &` | `[1] cask one`, then `no line at all (from a file or another program)` and, at the next prompt, that job 1 ended with code 1: in the background there is no input *(not tested)* |

**On the host**

```
make libc-test
```

The first group is now 11 string tests (one more: `strspn` and `strcspn`).

**Cleaning up**, in the OS: `rm typed who`.

Automated: `make test64`.
