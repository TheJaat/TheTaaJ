# Standard Output, Standard Input, and Redirection: a Program That Does Not Know Where Its Words Go

*Journal entry #31 · output and input become channels a program is handed; `>`, `>>` and `<` point them at files; and the shell stands at the other end*

This is the second step of the shell plumbing. I had planned it as two — "standard input and output as handles", then "redirection" — and did them as one, because the first has nothing to show for itself without the second. A program that prints through a handle instead of straight to the screen looks exactly the same from the outside until somebody points the handle somewhere else.

What works now:

```
taaj:/> ls docs > list            the listing goes into a file
taaj:/> echo one more >> list     added to the end
taaj:/> wc < list                 the file is the program's input
taaj:/> cat < big > copy          both at once
taaj:/> ps > procs                the shell's own commands too
```

and two new things on the disk: `cat` reads its input when given no names, and `wc` counts lines, words and bytes.

---

## The idea

`ls` prints a listing. Until now "prints" meant: sends text to the display service, and copies it to the kernel's debug log. That was written into the runtime every program links. So `ls` could show a listing on the screen and do nothing else with it — to save a listing I would have had to write a second program, `ls-to-file`.

Unix's answer, fifty years old and still the best idea in it: **a program does not print to the screen. It writes to "standard output" — something it is handed when it starts — and whoever starts it decides what that is.** The screen, a file, another program. `ls` is written once and never knows.

The same for input: a program reads "standard input" and does not know whether that is a file or the output of another program.

In TheTaaJ a thing a program is handed is a **handle**, and the natural thing for bytes to travel on is a **channel**. So:

- standard output is a channel the program sends on;
- standard input is a channel the program receives from;
- what is at the other end is not the program's business.

## The stream protocol

A channel carries messages. A stream of bytes has no messages in it — it is just bytes, in order. So there has to be an agreement about how one is carried on the other. It is in `abi`:

```rust
/// A stream: bytes in order, in messages, on a channel.
///
///   [DATA] [bytes]
///
/// A message may hold any number of bytes up to DATA_MAX, and where one
/// message ends means nothing - a reader must not expect a line, or a
/// whole anything, per message. The end of the stream is the sender
/// closing its end.
pub mod stream {
    /// The same byte as display::TEXT: text for the screen is a stream.
    pub const DATA: u8 = super::display::TEXT;
    pub const DATA_MAX: usize = super::MESSAGE_MAX as usize - 1;
}
```

Three decisions:

**Message boundaries mean nothing.** A sender may put a line in a message, or half of one, or twenty. A reader that assumed "one message, one line" would work with one sender and break with the next. `wc` has to count words correctly when a word is cut in half between two messages — it keeps a flag, `in_word`, from one piece to the next for exactly this.

**The end is the channel closing**, not a special message. When the sender exits — normally, or killed, or crashed — the kernel closes its handles, and the reader gets everything that was sent and then `PeerClosed` (entry #9 built channels this way). So a reader can never be left waiting for an "end" message that a crashed writer did not live to send. This is the same reason the file service's replies come on a fresh channel each time.

**`DATA` is the same byte as the display's `TEXT`.** The display service already took text as `[TEXT][bytes]`. Making the stream's tag the same number means the display *is* a stream reader, with no change to it: a program's standard output can be the display's channel or any other, and the program sends the same bytes either way.

## What a program does

```rust
pub fn write(bytes: &[u8]) -> Result<u64, Error> {
    match stdout() {
        Some(screen) if output_is_screen() => {
            write_to_display(screen, bytes);
            debug_write(bytes)
        }
        Some(output) => write_to_stream(output, bytes).map(|()| bytes.len() as u64),
        None => debug_write(bytes),
    }
}
```

`println!` ends up here, so no program changed how it prints. There are three cases, and the middle one is the new one.

### Why the screen is a special case at all

I wanted there to be no special case: output is a stream, send the bytes. Two things got in the way, and both are honest reasons.

**The debug log.** Every automated test I have reads the serial line. A program's output appears there because the runtime copies it there. If output redirected to a file were *also* copied to the log, no test could tell whether redirection worked — the text would be in the log either way. So the copy must happen only when the output is the screen. (In a finished system there would be no such copy at all. It is a testing aid and I am keeping it, with its eyes open.)

**What to do when the reader is stuck.** A channel holds 32 messages. A program that prints faster than the reader reads will find it full.

- For the **screen**, entry #19 decided: wait up to a second, then drop the text. A display that has hung must not hang every program that prints.
- For a **file**, dropping is unthinkable. A copy with a piece silently missing is worse than no copy. The writer must wait as long as it takes.

Those are opposite policies, and the program's runtime is the one that has to apply them. So it has to know which kind of output it has. One bit in the startup flags says so:

```rust
/// Not a handle - the one flag that is not ... : standard output is
/// the screen.
pub const SCREEN: u64 = 1 << 7;
```

The shell sets it when it hands a program the screen, and not when it hands it anything else.

```rust
pub fn write_to_stream(channel: Handle, bytes: &[u8]) -> Result<(), Error> {
    let mut message = [0u8; abi::MESSAGE_MAX as usize];
    message[0] = abi::stream::DATA;
    for chunk in bytes.chunks(abi::stream::DATA_MAX) {
        message[1..=chunk.len()].copy_from_slice(chunk);
        let mut tries = 0u32;
        loop {
            match send(channel, &message[..=chunk.len()], &[]) {
                Ok(()) => break,
                Err(Error::QueueFull) if tries < 64 => yield_now(),
                Err(Error::QueueFull) => sleep(1),
                Err(error) => return Err(error),
            }
            tries += 1;
        }
    }
    Ok(())
}
```

Full queue: step aside and try again, forever. Any other error — the reader closed its end — stop and say so.

This loop is the weakest thing in this entry and I want to be plain about it. **It is polling.** The right mechanism is "wake me when there is room", and the kernel does not have it: `object_wait` can wait for a channel to have a message, not for it to have space. Until it can, a writer facing a full channel asks again and again. It yields the CPU first (a reader with a full queue is ready to run, so yielding usually fixes it at once) and after 64 tries sleeps a tick between attempts, so it is not a busy loop — but it is a workaround, and it is on the list.

### Reading

```rust
pub struct Input {
    channel: Option<Handle>,
    held: Vec<u8>,
    taken: usize,
}

pub fn read(&mut self, out: &mut [u8]) -> usize   // 0 means the end
```

A message can bring a thousand bytes when the program asked for ten. `Input` keeps the rest for the next call — that is the whole reason it is a struct with state rather than a function. A program with no standard input gets "the end" immediately.

There is one trap in it I nearly fell into: **an empty message is not the end of the stream.** Returning 0 for a zero-length `DATA` message would tell the program its input was over when the sender had merely sent nothing. So `read` loops until it has bytes or the channel is closed.

### One program that does ask: `cat`

The first full test failed here:

```
FAIL: "big" is not the program ls twice over: cat ls ls > big lost or changed bytes
```

`big` was 50034 bytes; two copies of `ls` are 50032. Two extra bytes. Looking at where they were:

```
expected   … 00 00 00 00 7f 45 4c 46 …
got        … 00 00 00 00 0a 7f 45 4c 46 …
                         ^^ a newline nobody wrote
```

`cat` has had this line since entry #15:

```rust
// Leave the next line - the prompt - a line of its own.
Ok(size) if size > 0 && last != b'\n' => println!(),
```

If a file does not end in a newline, `cat` adds one, so the prompt does not appear stuck to the end of the file's last line. On the screen that is a courtesy. Into a file it is **corruption** — the copy has a byte the original does not. It had been harmless for sixteen entries because `cat`'s output had only ever been a screen.

Real Unix `cat` never adds anything; there the *shell* deals with an unfinished last line. I kept the courtesy, on the screen only:

```rust
Ok(size) if size > 0 && last != b'\n' && taaj::output_is_screen() => println!(),
```

`output_is_screen()` is what Unix calls `isatty()`. It is a function to be suspicious of, and the comment on it says so: a program that behaves differently depending on who is reading cannot be trusted in a pipeline. The fair use is decoration — like this, or colours — that would be noise anywhere else.

I am glad this was caught by comparing bytes, on a binary file. A text test (`cat notes > copy`, then look at it) would have passed: the files I test with all end in newlines.

## What the shell does

### Reading the line

`>`, `>>` and `<` are understood by the same pass that understands quotes (entry #30), and for the reason I gave there:

```
echo "a > b"        prints  a > b
echo a > b          writes  a  into the file b
```

After quotes are removed those two lines have the same words. Only the code that sees the quotes can tell an instruction from text. So the splitter now produces *tokens* — a word, or a redirection — and a second small step turns them into

```rust
pub struct Command {
    pub words: Vec<String>,
    pub output: Option<(String, bool)>,   // the file, and whether to add to its end
    pub input: Option<String>,
}
```

They need no spaces (`ls>list`), may stand anywhere on the line, and are not arguments — the program never sees them. Refused: a `>` with no name after it, and two outputs or two inputs.

Telling `>` from `>>` needs to look one character ahead without taking it:

```rust
let mut characters = line.chars().peekable();
...
_ if characters.next_if_eq(&'>').is_some() => Redirect::Append,
_ => Redirect::Out,
```

`peekable()` wraps an iterator so it can be looked into; `next_if_eq` takes the next item only if it equals the one named.

### The shell is the other end

Here is the decision that shaped everything else. When I type `ls > list`, who writes `list`?

| | How | Problem |
|---|---|---|
| **Give `ls` the file** | The file service hands out a channel for one open file, and `ls` gets it as its output | The file service would hold state per open file. It is stateless and restartable by design (entries #23–#26) |
| **Give `ls` the file service** | `ls` appends to the file itself | Every program would need to know about files to be redirected — the opposite of the point — and would hold authority over the whole disk |
| **The shell writes it** | `ls` gets a plain channel; the shell reads the other end and appends to the file | The shell is in the path of every byte |

I chose the third.

```
        ls                    shell                       fs
        ──                    ─────                       ──
   println!(...) ──DATA──►  receive                     
                            files.append("list", …) ───► WRITE (one commit)
   println!(...) ──DATA──►  receive
                            files.append("list", …) ───► WRITE
   exit  ─── channel closes ─► PeerClosed: done
```

What I like about it is the **authority**. `ls` is granted the read-only file channel. With `ls > list` it still has only that. It was handed a channel it can send text on — it does not know a file is involved, could not name the file, and cannot write anywhere else. The *user* said where the output goes, the *shell* — which holds the user's authority — did the writing. Redirection hands a program no power it did not have. (Compare entry #21's rule: a program gets what its job needs.)

This is also the one place the shell changes files itself. Its header comment used to say "the shell itself never changes a file"; it now says when it does.

Input is the mirror image: the shell reads the file a piece at a time and sends the pieces down a channel whose other end is the program's standard input.

### Keeping both ends moving

The shell already had a loop that waits for the foreground program while watching the keyboard for Ctrl+C (entry #17). It now has up to four things to attend to:

```rust
let watched = [
    (Some(process), Source::Process),
    (keys, Source::Keys),
    (output.as_ref().map(|(handle, _)| *handle), Source::Output),
];
```

plus the input feed. Each time round:

1. **Feed**: send as many pieces of the input file as the program's channel has room for.
2. **Wait** for whichever comes first: the program ending, a key, or output arriving.
3. **Output arrived**: take one message, append it to the file.
4. **The program ended**: take whatever output is still in the channel, close everything, done.

Step 4 has an order that matters. A program can print its last lines and exit before the shell has read them. Its exit and its last output are *both* ready. If the shell saw "ended" and stopped, the end of every redirected file would be cut off at random. So on "ended" the shell first drains what is already in the channel — `poll`, take, repeat — and only then finishes. Nothing more can arrive after the process is gone, so draining "what is there now" is exactly enough.

**Why this is not allowed to stall.** If the shell stopped reading, the program's channel would fill at 32 messages and the program would wait forever (by the rule above — nothing is dropped). And if the program stopped reading its input while the shell insisted on sending it, the *shell* would be stuck. So the shell never blocks on the input: if the channel is full it keeps the piece (`Feed` holds one unsent message) and goes on to the wait. The test for all of this is `cat < big > big2` with a file of 50 messages — more than a channel holds — so that both ends really do have to wait for each other.

The same polling weakness as before appears here: with input still to send, the shell cannot wait for "room in the channel", so it waits one tick and tries again.

```rust
let ready = match &feed {
    Some(_) => taaj::object_wait_until(&handles[..count], taaj::ticks() + 1),
    None => taaj::object_wait(&handles[..count]).map(Some),
};
```

A little Rust that made the feed pleasant to write: the closure that copies a piece into the waiting message borrows *only* `feed.message`, so `feed.path` and `feed.offset` can be read in the same call. Since the 2021 edition a closure captures the individual fields it uses, not the whole struct.

### When the file is made

```rust
let made = if *append { files.append(&path, &[]) } else { files.write(&path, &[]) };
```

The output file is created, or emptied, **before** the command runs — as in every Unix shell. Two consequences, both traditional and both worth knowing:

- `ls > list` shows `list` in its own listing, with size 0.
- `cat < f > f` empties `f` before `cat` reads it. The result is an empty file.

One ordering I did choose: the input file is checked *first*. `wc < nosuch > out` fails on `nosuch` and leaves `out` untouched, rather than emptying `out` and then discovering there is nothing to read.

### The shell's own commands

`echo`, `help`, `ps`, `pwd`, `mem` are not programs. Their output goes through `Shell::write`, which now looks at the line's redirection:

```rust
fn write(&self, bytes: &[u8]) {
    let output = self.redirection.borrow().output.clone();
    match output {
        Some(path) => self.to_file(&path, bytes),
        None => self.tell(bytes),
    }
}
```

So `echo hello > notes` — surely the most-typed redirection there is — works, and `write notes hello` is now just one way of doing it.

### What does *not* go into the file

There are two kinds of text when a command runs: what the command produces, and what the shell has to say *about* the command — `unknown command`, `cat exited with code 1`, `^C`, `forever was stopped`. The second kind now goes through `tell`, always to the screen:

```
taaj:/> nosuch > empty
unknown command: nosuch (try help)
```

The alternative is a file containing "unknown command" and a screen showing nothing, which is a miserable way to find a typo.

But note the limit of this. The *program's own* complaints are a different matter:

```
taaj:/> cat none > err
cat exited with code 1
taaj:/> cat err
cat: none: no such file or directory
```

`cat`'s error message went into the file, because `cat` has one output and that is where it prints. Unix solved this with a second output, **standard error**, which stays on the screen when standard output is redirected. I do not have one yet. It is the first thing on the missing list, and the boot test checks the current behaviour (the complaint *is* in `err`) so that when I add it, the test has to change with it.

## `wc`

A new 80-line program, mostly as something that *reads* input:

```
taaj:/> wc long
long: 300 lines, 1200 words, 4692 bytes
taaj:/> wc < long
300 lines, 1200 words, 4692 bytes
```

Same counts, by two routes: the first asks the file service for the file by name, the second is handed the bytes and knows nothing of files. With `<` it needs no file permission at all.

## The design, in one table

| Decision | Chosen | Rejected, and why |
|---|---|---|
| What standard output is | A channel carrying a tagged byte stream | A file handle from the file service — per-open-file state in a stateless service |
| End of stream | The channel closing | An "end" message — a crashed writer never sends it |
| Who writes a redirected file | The shell, with its own authority | The program — it would need the file service, and the power that goes with it |
| A full channel, output is a file | Wait, forever | Drop — a copy with a hole in it |
| A full channel, output is the screen | Wait a second, then drop (unchanged) | Wait forever — a hung display would hang every program |
| How the runtime knows which | One startup flag, `SCREEN` | Guessing; or copying everything to the log, which blinds the tests |
| Parsing `>` | In the same pass as quotes | On the finished words — cannot tell `">"` from `>` |
| Shell's remarks about a command | Always to the screen | Into the file with the output |
| Scope | The whole line (`time ls > x`) | Just the innermost command |

## The bugs

1. **`cat`'s extra newline** — above. Found by a byte comparison of a binary file.
2. **My own arithmetic.** I expected `wc long` to say 1500 words. `line 7 of 300` is four words, not five; the OS said 1200 and the OS was right.
3. **A check that asked the wrong question.** To confirm `nosuch > empty` left an empty file I ran the host tool's `ls` on the *file* `empty`, where it expects a directory. Wrong check, right OS.

Two of the three "failures" in this entry were the test being wrong. That is not a reason to trust tests less; it is what tests written before the first run look like. But it is worth saying, because each took a look at the actual bytes on the disk to settle — "the test failed" is where the work starts.

## How I checked it

**On the host** (`make kernel64-test`):

- `input`: 4 new tests of redirections in a line (22 in the crate) — every form, with and without spaces, at either end of the line; quoted signs staying text; a file *named* `>`; and the lines that must be refused.

**In the boot test** (BIOS and UEFI), 18 new lines are typed. After QEMU exits the host tool reads the disk and must find:

| File | Must be | Shows |
|---|---|---|
| `out` | `first`, `second` | `>` then `>>` from the shell's own `echo` |
| `both` | `welcome` followed by `out` | a program's output; `cat` joining files |
| `procs` | contains `init … running` | a built-in's multi-line output |
| `long3` | **byte-identical** to `long` | `cat < long > long3`: input and output together |
| `big` | **byte-identical** to `ls` twice | 50 KB of binary through an output |
| `big2` | **byte-identical** to `big` | 50 messages in and 50 out, more than a channel holds |
| `err` | `cat: none: no such file or directory` | a program's complaint follows its output |
| `empty` | exists, 0 bytes | the file is made even when the command is not found |

And on the screen: `wc < list` counting a redirected listing (6 lines), the two `wc long` forms agreeing, `echo "a > b"` printing its `>`, and four refusals.

The check that `ps` reports the same number of processes, threads and channels at the start and end of the test still passes — after all of these redirections, no channel was leaked.

`make test64` passed.

**Not tested:**

- I have not typed any of this into a `make run64` window myself.
- **Ctrl+C during a redirected command.** The code path is the old one plus closing two channels; nothing exercises it.
- **A disk that fills up during `>`.** The shell should stop writing, let the program finish, and report `the file system is full`. Written, never run.
- **The file service restarting in the middle of a redirection.**
- **How long it takes.** Every message appended is its own commit to the disk. The 50 KB test finishes within the test's time limits; I have not measured it, and it is certainly slow.
- **`>` with nothing to run** (`> file` alone) should make an empty file; host-tested as a parse, not typed.

## What is still missing

- **No standard error.** A program's complaints go into the file with its output.
- **No keyboard as standard input.** A program run without `<` has no input at all; `cat` with no arguments cannot read what I type. That needs the shell to pass keys on, with echo and line editing — a terminal's "line discipline" — and an end-of-input key (Ctrl+D).
- **No way to wait for room in a channel.** Writers and the shell's feed poll. The kernel needs a "writable" readiness for `object_wait`.
- **One commit per message** when writing a redirected file. The shell should gather output and write it in larger pieces.
- **No pipes** (`ls | wc`) — the next step, and now mostly a matter of connecting one program's output channel to another's input instead of to the shell.
- **`write FILE TEXT` and `append`** are now redundant with `echo TEXT > FILE`. I am keeping them until the tests no longer lean on them.
- **No "replace only when complete"**: a `>` that is interrupted leaves a short file.

## What I learned

- **"Standard output" is an act of ignorance, and that is its value.** The program not knowing where its output goes is what lets one `cat` show, copy and join.
- **A byte stream on a message channel needs a rule about boundaries, and the rule should be "they mean nothing".** Any other rule works until the second writer.
- **Let the transport signal the end.** A closed channel cannot be forgotten by a dying process.
- **Whoever holds the authority should do the act.** Having the shell write the file keeps redirection from being a way to hand out power.
- **The same full queue wants opposite answers** for a screen and for a file, and seeing that is what forced the one flag I had hoped not to need.
- **A kindness in one setting is damage in another.** `cat`'s newline. Compare bytes, and use a binary file.
- **Drain before you finish.** "The process ended" and "its last output arrived" are separate events that become ready together.
- In Rust: `peekable` and `next_if_eq`; `Option::replace` to detect "already set"; `RefCell::take`; closures capturing single fields; arrays of `Option` iterated with `flatten`; and an `enum` declared inside the function that is its only user.

---

## Testing note

```
make run64
```

`make run64` puts the new program `wc` on your disk. Start in the root (`cd`). `>` is Shift+`.` and `<` is Shift+`,`.

**Output into a file**

| Type | You should see |
|---|---|
| `echo first > out` | Nothing printed |
| `echo second >> out` | Nothing |
| `cat out` | `first` then `second` |
| `echo third > out` then `cat out` | `third` only — `>` replaces *(not in the automated test)* |
| `ls>list` | Nothing — no spaces needed *(the automated test types `ls docs>list`)* |
| `cat list` | The listing of the root, **including** a line `       0  list` *(the `cat` is not in the automated test)* |
| `ps > procs` then `cat procs` | The process table, from the file *(the `cat` is not automated; the host reads the file)* |
| `pwd > where` then `cat where` | `/` *(not in the automated test)* |

**Input from a file**

| Type | You should see |
|---|---|
| `wc list` | `list: N lines, N words, N bytes` |
| `wc < list` | The same three numbers, with no name in front |
| `cat < out` | What `out` holds *(not in the automated test)* |

**Both at once**

| Type | You should see |
|---|---|
| `cat < list > list2` | Nothing |
| `wc list list2` | Two lines with the same numbers *(not in the automated test)* |
| `cat ls ls > big` | Nothing, after a pause — it is 50 KB, written a message at a time |
| `cat < big > big2` | Nothing, after a longer pause |
| `ls big big2` | Both the same size: twice the size `ls ls` shows *(not in the automated test)* |
| `cat welcome out > both` then `wc welcome out both` | `both`'s numbers are the sums of the other two *(the `wc` is not in the automated test)* |

**A `>` that is only text**

| Type | You should see |
|---|---|
| `echo "a > b"` | `a > b` — and no file `b` is made |
| `echo a \> b` | `a > b` *(host-tested only)* |

**What is refused, and what goes where**

| Type | You should see |
|---|---|
| `wc < none` | `none: no such file or directory` |
| `echo x > docs` (any directory) | `docs: is a directory` |
| `echo x >` | `there is a > with no file name after it` |
| `echo x > a > b` | `the output can go to only one file` *(host-tested only)* |
| `nosuch > empty` | `unknown command: nosuch (try help)` — on the screen, not in the file |
| `ls empty` | `       0  empty` — the file was made all the same *(the `ls` is not automated)* |
| `cat none > err` | `cat exited with code 1` — and nothing else |
| `cat err` | `cat: none: no such file or directory` — the program's complaint went with its output *(the `cat` is not automated)* |

**`help`** — still one screen, now with `COMMAND > FILE` and `COMMAND < FILE`.

**From the host** — with QEMU closed:

```
tools/tfstool-rs/target/release/tfstool get build/taaj-disk.img out
tools/tfstool-rs/target/release/tfstool get build/taaj-disk.img big  | cmp - <(cat user/target/x86_64-unknown-none/release/ls user/target/x86_64-unknown-none/release/ls) && echo same
tools/tfstool-rs/target/release/tfstool get build/taaj-disk.img big2 | cmp - <(tools/tfstool-rs/target/release/tfstool get build/taaj-disk.img big) && echo same
```

The first prints what `out` holds; the other two print `same`.

**Cleaning up**, in the OS: `rm out list list2 procs where big big2 both empty err`.

Automated: `make test64`.
