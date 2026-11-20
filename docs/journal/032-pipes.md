# Pipes: Two Programs, One Channel, and a Shell That Lets Go

*Journal entry #32 · `A | B` joins commands: what A prints is what B reads; the shell makes the channel, gives both ends away, and waits for everyone at once*

The third step of the shell plumbing, and the one the last entry was really for. After entry #31 a program writes to a standard output it is handed and reads a standard input it is handed, and knows nothing about what is at the other end. A pipe is what happens when the other end is *another program*.

```
taaj:/> cat notes | wc                  how long is it
taaj:/> cat ls ls | cat | cat > copy    three programs, 50 KB between them
taaj:/> cat < long | wc >> counts       a file in at the start, a file out at the end
taaj:/> ps | wc                         the shell's own command first
taaj:/> yes | given                     a writer that never stops - and stops
```

There is also a new program on the disk, `yes`, which exists to be the writer that never stops.

---

## The idea

`wc` counts what it reads. `cat` prints files. Neither knows the other exists. Joined by a pipe they count the lines in several files together — a thing nobody wrote a program for.

That is the whole argument for pipes: **small programs that each do one thing become a toolkit, if the output of any can be the input of any.** It only works because of the last entry's rule — a program must not know or care what its output is.

## What a pipe is here

In Unix a pipe is a kernel object of its own: a buffer with a reading end and a writing end. Here I needed nothing new. A stream is already "messages on a channel, the end is the sender closing" (entry #31), and a channel already has two ends. So:

```
            shell:  (send, receive) = channel_create()

      cat                                   wc
  standard output  ═══ one channel ═══►  standard input
      (send)                              (receive)
```

For `A | B` the shell makes one channel, gives the sending end to A as its standard output and the receiving end to B as its standard input. For `A | B | C`, two channels. That is all a pipe is.

Compare with redirection, where the shell stood at the far end of the program's channel and carried every byte to the file. In a pipe **the shell is not in the path at all**. The bytes go from `cat` to `wc` and the shell never sees them.

### The shell must keep nothing

This is the one rule that cannot be bent, and the reason is the definition of "end of stream".

`wc` stops reading when its input ends. Its input ends when the channel's sending side is closed. The sending side is closed when *every handle to it* is closed. If the shell kept a copy of the sending end "just in case", then after `cat` had finished and gone, the channel would still have a sender — the shell — and `wc` would wait for ever for bytes the shell was never going to send.

This is the classic bug of every first shell written on Unix (forgetting to `close()` the pipe's write end in the parent), and the design here makes it hard to commit: sending a handle in a message *moves* it. When the shell sends the channel end to init, it leaves the shell; when init spawns the program with it, it leaves init. Nobody has to remember to close anything, because nobody has it any more.

## Starting them, and waiting for all of them

```rust
let mut carried: Option<Handle> = None;     // the previous program's reading end
for (n, command) in programs.iter().enumerate() {
    let stdin = match carried.take() {
        Some(previous) => Ok(Some(previous)),
        None => self.input_end()...          // the first: the line's input, if any
    };
    let stdout = if n + 1 < programs.len() {
        taaj::channel_create().map(|(send, receive)| {
            carried = Some(receive);         // for the next one
            Output::Channel(send)
        })
    } else {
        self.output_end()...                 // the last: the file, or the screen
    };
    let process = self.start(name, arguments, image, Grant::Usual, stdout, stdin);
    ...
}
```

`carried` is the whole trick: each turn of the loop makes the channel to the *next* program, hands over the sending end now, and carries the receiving end into the next turn.

### They must run at the same time

Could the shell run `cat` to its end, then `wc`? No: a channel holds 32 messages. `cat ls ls` sends 50. With nobody reading, `cat` would fill the channel and wait for room for ever (entry #31: a stream never drops bytes). The programs of a pipeline have to be alive together, each taking what the one before it gives. That is what makes a pipe a pipe and not a temporary file.

So the shell starts them all, and then waits for them all:

```rust
fn wait_for(&self, processes: &[Handle], output: Option<Handle>, feed: Option<Feed>)
    -> Result<Vec<i64>, Error>
```

The loop from entry #31 watched one process, the keyboard, and the output channel. Now it watches every process that has not ended. When one ends, its exit code is noted and it leaves the watch list; when all have ended, the output still in the channel is drained (the rule from last time) and the wait is over.

A piece of Rust I enjoyed here. The codes are a `Vec<Option<i64>>` — `None` while a program is running. "Have they all ended?" is

```rust
if let Some(codes) = codes.iter().copied().collect::<Option<Vec<i64>>>() {
```

Collecting an iterator of `Option`s *into* an `Option<Vec>` gives `Some(vec)` only if every item was `Some`, and `None` at the first `None`. One expression answers the question and unwraps the answers.

**Ctrl+C** stops every program of the pipeline still running, not one of them.

### Find them all before starting any

```
taaj:/> rm notes | wcc
```

A typo in the *second* command. If the shell started the programs as it went, `rm` would have run — and removed the file — before the shell found there is no `wcc`. So every program is looked up and read from the disk first, and only when all are there is the first one started:

```
unknown command: wcc (try help)
```

and `notes` is untouched.

## When the reader leaves first

```
taaj:/> yes | given
```

`yes` prints `y` for ever. `given` says what it was started with and exits, without reading a byte. What should happen to `yes`?

Nothing is reading. Everything `yes` writes from now on is for nobody. And it will never stop on its own. Unix's answer is a signal: a write to a pipe with no reader delivers `SIGPIPE`, and the default action of that signal is to end the process. I do not have signals, but I have what matters — the kernel tells a sender when the other end is closed: `send` fails with `PeerClosed`.

So the question is who acts on it. The program could: `println!` returns... nothing, and behind it `taaj::write` returns a `Result` that not one of my programs looks at. Leaving it to each program means every program gets it wrong. So the runtime does it, in the one place every write goes through:

```rust
Some(output) => match write_to_stream(output, bytes) {
    Ok(()) => Ok(bytes.len() as u64),
    Err(Error::PeerClosed) => exit(abi::NO_READER),
    Err(error) => Err(error),
},
```

A program that writes to a stream nobody reads ends there, with an exit code set aside for it:

```rust
/// Exit code of a program that ended itself because nobody was reading
/// its standard output any more ...
pub const NO_READER: i64 = -1002;
```

and the shell, which reports any other non-zero exit, says nothing about this one — it is not a failure, it is how `yes | given` is meant to end. From the test's log:

```
[process] 122 "given" exited with code 0
[process] 121 "yes" exited with code -1002
```

### Why I had to write `yes`

My first idea for the test was `forever | given`: I already had a program that never ends. It would have hung the test. `forever` prints one line and then computes without ever printing again — and **a program finds out its reader has gone only when it next writes.** A silent program in a pipeline whose reader has left runs on until Ctrl+C. That is true in Unix too (`sleep 1000 | true` waits the full thousand seconds), and it is not a bug, but I had not seen it clearly until my test program turned out to be the wrong one.

## The shell's own commands

`ps | wc` ought to work. But `ps` is not a program: it is code in the shell. It cannot "run at the same time" as `wc` — the shell has one thread, and while it is printing the process table it is not doing anything else.

| | How | Problem |
|---|---|---|
| **A second shell process** | Start a copy of the shell to run `ps`, as Unix does (`fork`) | There is no fork here, and a new shell would have none of this one's state |
| **Write into the pipe directly** | The shell sends `ps`'s output down the channel as it is produced | 32 messages and it is full, with the shell itself stuck — and it is the shell that would have to keep everything else moving |
| **Run it first, keep the output** | `ps` runs to its end with its output collected in memory; that is then sent to `wc` the way a file is | Not concurrent; the output has to fit in memory |

I chose the third. The mechanism was nearly there: last entry the shell's own output could go to a file instead of the screen. Now there are two places it can go instead —

```rust
enum Sink {
    File(String),      // added to the end of this file
    Memory,            // kept, for the next command of a pipeline
}
```

— and the thing the shell sends down a program's input can come from two places:

```rust
enum Supply {
    File(String),      // `< file`
    Memory(Vec<u8>),   // what one of the shell's own commands printed
}
```

`ps | wc` is: set the sink to memory, run `ps`, take what was kept, and start `wc` with that as its supply. `wc` cannot tell it from `wc < file`.

Two limits come with this, both stated rather than hidden:

- **One of the shell's own commands can only be first.** They read no input, so anywhere else there would be nothing for them to do with what arrives:
  ```
  taaj:/> ls | cd
  cd is one of the shell's own commands: it reads nothing, so it can only begin a pipeline
  ```
- **At most 1 MiB is kept.** More is dropped, and the shell says so. Without a limit, `time yes | wc` would grow until the shell ran out of memory, and the shell is the last program that should die of a typo.

`time`, `run` and `with` are the shell's own commands that *start a program*. They work first in a pipeline too — `time cat long | wc` — because the program they start has its output collected exactly as a redirected program's was last time.

## Reading the line

`|` is one more token for the splitter of entries #30 and #31, and it is text when quoted (`echo "p | q"`). The parser now returns a list of commands instead of one:

```rust
pub fn parse(line: &str) -> Result<Vec<Command>, LineError>
```

Two things it refuses.

**A side with no command**: `ls |`, `| wc`, `ls | | wc`.

**A file where there is already a neighbour.** In `a | b | c`, only `a` has a free input and only `c` a free output:

```
taaj:/> ls > x | wc
only the last command of a pipeline can send its output to a file
```

A Unix shell accepts that line: the file wins, and `wc` gets an empty input. I cannot think of a time anyone meant it. Refusing costs nothing and says what is wrong.

## The refactor underneath

`run_program` was one function that found the program, built its channels, asked init to start it, waited, and reported. A pipeline needs "start" several times and "wait" once, so it came apart into pieces that both paths use:

| Piece | Does |
|---|---|
| `program_image` | finds the file and reads it (unchanged) |
| `output_end`, `input_end` | make the channel for the line's output or input, if it has one |
| `start` | asks init to start one program with a given output and input; returns the process, does not wait |
| `wait_for` | waits for any number of processes, moving output and input meanwhile |
| `report` | says how one ended, if that needs saying |

`run_program` is now those five in a row, for one program. `run_pipeline` is the loop above around `start`.

One rule made `start` simple to use: **it takes ownership of what it is given** — the image and both channel ends — whatever happens. If the program cannot be started, `start` closes them. The caller never has to work out which handles are still its own after a failure, which is exactly the kind of bookkeeping that leaks a channel on the one path nobody tests.

## The design, in one table

| Decision | Chosen | Rejected, and why |
|---|---|---|
| What a pipe is | A plain channel, one end to each program | A new kernel object — nothing it would add |
| The shell's part | Makes the channel, keeps neither end | Copying bytes between two channels — slower, and one more place to stall |
| Order of running | All started, all waited for together | One after another — more than 32 messages would never fit |
| A mistyped command | Every program is found before any is started | Start as you go — the earlier ones run anyway |
| The reader leaves | The writer's runtime ends it at its next write, with `NO_READER` | Each program checking its writes — none do |
| The shell's own commands | First only; run to the end, output kept (1 MiB) | Anywhere — they read nothing; unlimited — the shell runs out of memory |
| `a > f \| b` | Refused | The file wins and `b` reads nothing, as in Unix — never what was meant |
| Reporting | Each program that failed is named; `NO_READER` is not a failure | One status for the pipeline |

## The bugs

This one was unusually quiet: the first boot with pipes passed everything, including the 50 KB through three programs. I do not think that is luck — the hard parts (streams with no message boundaries, draining before finishing, feeding without blocking, the byte-exact tests) were all found and fixed in entry #31, and a pipe is those same parts connected differently.

What did go wrong went wrong before it ran:

1. **The wrong test program** — `forever | given` would have hung, as told above. Caught by thinking through what `forever` does, not by a test.
2. **A closure that could not return a borrowed `&str`.** I wrote a small closure to get a command's name, made it return an owned `String` to keep the compiler quiet, and then had `.as_str()` scattered everywhere. A plain `fn name(command: &Command) -> &str` does it properly: a function's signature can say "the result borrows from the argument", which a closure's cannot easily.

## How I checked it

**On the host** (`make kernel64-test`): 4 new tests in `input` (26 in the crate) — commands joined with and without spaces; a pipeline of one; files at the start and the end, and refused in the middle; every way of leaving a side empty; a quoted `|` as text.

**In the boot test** (BIOS and UEFI), 11 new lines are typed:

| Typed | Checked |
|---|---|
| `cat long \| wc > piped` | the host reads `piped` from the disk afterwards |
| `cat < long \| wc >> piped` | `piped` must be `300 lines, 1200 words, 4692 bytes`, twice |
| `cat ls ls \| cat \| cat > big3` | `big3` must be **byte-identical** to the program `ls` twice over |
| `echo one two three \| wc` | `1 line, 3 words, 14 bytes` on the screen |
| `time cat long \| wc` | `301 lines, 1203 words` — the file, plus the line `time` adds |
| `yes \| given xpipe` | `given` answers; and the test goes on, so `yes` ended |
| `echo "p \| q"` | `p \| q` |
| `ls \|`, `ls \| cd`, `ls \| nosuch2`, `ls > x \| wc` | the four refusals |

And the check from earlier entries that `ps` reports the same number of processes, threads and channels at the start and the end of the test still passes: after all of this no process is left running (so `yes` really went) and no channel was leaked.

`make test64` passed.

**Not tested:**

- I have not typed any of this into a `make run64` window myself.
- **Ctrl+C in a pipeline.** It should stop every program still running. Written, not exercised.
- **A program that cannot be started in the middle of a pipeline** (init refusing, or no channel to be had). The ones already started are stopped and everything is closed — on paper. The common case, a mistyped name, never gets that far and *is* tested.
- **More than 1 MiB from one of the shell's own commands**, and **more than 8 commands**: both refusals are written and never run.
- **That `rm notes | wcc` leaves `notes` alone**: the test shows nothing was started for `ls | nosuch2` (the log has no `[init] started ls`), which is the same code, but not with a command that changes something.

## What is still missing

- **Background jobs** — `forever &`, `jobs`, `kill` — the last step of the plumbing, and next.
- **The shell's own commands anywhere but first**, and running concurrently. `yes | echo done` cannot be typed.
- **A silent writer outlives its reader** until Ctrl+C. Unix has the same behaviour; a kernel that told a *sender* when its peer closed (as an event to wait on) would let a runtime do better.
- **No standard error**, still: in `cat none | wc`, `cat`'s complaint goes down the pipe and `wc` counts it.
- **No exit status for a pipeline**, or for anything: there is no `&&`, no `if`, nothing that reads how a command ended.
- **Writers still poll** on a full channel (entry #31). In a pipeline that is two programs taking turns by yielding — it works, and it is wasteful.
- **`with files A | B`** grants only when it is first; there is no way to say it for a later command.

## What I learned

- **A pipe is not a thing; it is an arrangement.** With streams defined on channels there was nothing left to invent — only two ends to hand to two programs.
- **End-of-stream is a property of every handle, not of the writer.** One forgotten copy of the sending end and the reader waits for ever. Handles that *move* when sent make the right thing the default.
- **A program learns its reader is gone only by writing.** Which is why the runtime must act on it, and why a silent program does not notice.
- **Decide what a function owns.** "`start` closes what it was given if it fails" removed every question about cleanup at its call sites.
- **Look everything up before doing anything**, when half-done is worse than not done.
- **Work done in the right place is not repeated.** This entry had no runtime bugs because the last one had all of them.
- In Rust: collecting `Option`s into an `Option<Vec>`; `unzip` to build two `Vec`s from one iterator of pairs; `core::mem::take` to move a value out and leave an empty one; `Option::replace`; `matches!`; and that a named function can return a borrow of its argument where a closure fights you.

---

## Testing note

```
make run64
```

`make run64` puts the new program `yes` on your disk. Start in the root (`cd`). `|` is Shift+`\`.

**Two programs**

| Type | You should see |
|---|---|
| `cat welcome \| wc` | `1 line, 4 words, 20 bytes` *(not in the automated test)* |
| `ls \| wc` | A count: one line for each line `ls` prints *(not in the automated test)* |
| `cat welcome welcome \| wc` | `2 lines, 8 words, 40 bytes` *(not in the automated test)* |
| `ls\|wc` | The same as `ls \| wc` — no spaces needed *(host-tested only)* |

**More than two, and files at the ends**

| Type | You should see |
|---|---|
| `cat ls ls \| cat \| cat > big3` | Nothing, after a pause — 50 KB through three programs |
| `ls ls big3` | `big3` is exactly twice the size of `ls` *(the automated test compares the bytes from the host instead)* |
| `cat welcome \| wc > piped` | Nothing |
| `cat < welcome \| wc >> piped` | Nothing |
| `cat piped` | `1 line, 4 words, 20 bytes`, twice *(the automated test does this with a longer file, and reads `piped` from the host)* |

**The shell's own commands, first**

| Type | You should see |
|---|---|
| `echo one two three \| wc` | `1 line, 3 words, 14 bytes` |
| `ps \| wc` | A count: as many lines as `ps` prints *(not in the automated test)* |
| `help \| wc` | `32 lines, …` *(not in the automated test)* |
| `time cat welcome \| wc` | `2 lines, 7 words, …` — the file's line, and the line `time` adds *(the automated test uses a longer file)* |

**A writer that never stops**

| Type | You should see |
|---|---|
| `yes \| given` | `given: the screen, no files`, and then the prompt — `yes` ended by itself, with nothing said |
| `ps` | No `yes` in the list *(the automated test checks the count of processes)* |
| `yes` | `y` down the screen until Ctrl+C, then `^C` and `yes was stopped` *(not in the automated test)* |
| `yes \| wc`, then Ctrl+C | `^C`, `yes was stopped`, `wc was stopped` — both *(not tested at all: please try it)* |

**A `|` that is only text**

| Type | You should see |
|---|---|
| `echo "p \| q"` | `p \| q` |

**What is refused**

| Type | You should see |
|---|---|
| `ls \|` | `there is a \| with no command on one side of it` |
| `\| wc` | The same *(host-tested only)* |
| `ls \| cd` | `cd is one of the shell's own commands: it reads nothing, so it can only begin a pipeline` |
| `ls \| nosuch2` | `unknown command: nosuch2 (try help)` — and `ls` was not run |
| `ls > x \| wc` | `only the last command of a pipeline can send its output to a file` — and no file `x` is made |
| `ls \| wc < x` | `only the first command of a pipeline can take its input from a file` *(host-tested only)* |

**`help`** — still one screen, now with `A | B` and `yes`.

**From the host** — with QEMU closed:

```
tools/tfstool-rs/target/release/tfstool get build/taaj-disk.img piped
tools/tfstool-rs/target/release/tfstool get build/taaj-disk.img big3 | cmp - <(cat user/target/x86_64-unknown-none/release/ls user/target/x86_64-unknown-none/release/ls) && echo same
```

The first prints the two counts; the second prints `same`.

**Cleaning up**, in the OS: `rm big3 piped`.

Automated: `make test64`.
