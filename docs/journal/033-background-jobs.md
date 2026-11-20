# Background Jobs: Everything the Shell Runs Is a Job, and One Loop Looks After Them All

*Journal entry #33 · a line ending in `&` is started and not waited for; `jobs` lists them, `kill N` ends one, `wait` waits for them; and the shell's two waiting loops become one*

The last step of the shell plumbing. Until now the shell did one thing at a time: it ran a command, waited for it to end, and only then read the next line. A program that takes a minute held the shell for a minute.

```
taaj:/> cat ls ls | cat > big4 &
[1] cat ls ls | cat > big4
taaj:/> forever sleep &
forever: sleep until stopped (Ctrl+C)
[2] forever sleep
[1] done  cat ls ls | cat > big4
taaj:/> jobs
[2] running  forever sleep
taaj:/> kill 2
[2] stopped  forever sleep
```

---

## The idea

"Run it in the background" sounds like a feature of the program. It is not — the program is exactly the same. **Background is something the shell does not do: wait.** Programs here have always been separate processes running on their own; the shell *chose* to sit still until each one ended. `&` means: do not sit still.

Put that way the feature looks like deleting a line. It was not, and the reason is the last two entries.

## What the shell had been doing while it "waited"

After entry #31 the shell's wait is not idle. With `ls > list` the shell is the far end of `ls`'s output channel, and it is the shell that adds every arriving message to the file. With `wc < list` the shell sends the file down a channel a piece at a time. That work was done inside the loop that waited for the program.

So for `cat ls ls | cat > big4 &` — who writes `big4`? The shell has gone back to its prompt. If nobody takes what the second `cat` prints, its channel fills at 32 messages and it stops for ever (a stream never drops bytes; entry #31).

The shell had two places where it waits:

| Where | Waiting for | Also doing |
|---|---|---|
| `wait_for` | the foreground program to end | moving its output and input; Ctrl+C; keeping typed keys |
| `next_key` | a key | nothing |

A background job with a file at either end has to be served **in both** — while I type, and while some other command runs in the foreground. Two loops would each need to know about every job. That is the kind of duplication where one copy gets a fix and the other does not.

## One list, one loop

So the shell now has one list of everything it has running, and a foreground command is not a special case of it:

```rust
struct Job {
    /// 1, 2, ... for a background job. 0 is the job in the foreground.
    number: usize,
    text: String,                 // the line, for `jobs`
    names: Vec<String>,           // its programs ...
    processes: Vec<Handle>,       // ... their handles ...
    codes: Vec<Option<i64>>,      // ... and exit codes: None while running
    output: Option<Handle>,       // our end of its output channel, if not the screen
    sink: Option<String>,         // the file that output is added to
    feed: Option<Feed>,           // the input we are sending it, if any
    failed: Option<FileError>,
}
```

**The foreground job is a job the shell happens to be waiting for.** That sentence is the design. `wait_for`, which used to be a hundred-line loop, is now: put a job numbered 0 in the list, and ask the one loop to run until it is over.

And one loop serves the list:

```rust
enum Until {
    Foreground,   // the foreground job being over
    Key,          // a key
    Jobs,         // every background job being over: the command `wait`
}

fn attend(&self, until: Until, input: &mut [u8; 16]) -> Result<Attended, Error>
```

Each turn, for **every** job: finish with it if its programs have all ended; send what input its channel will take; then wait on *all* the things that can happen — the keyboard, every running process of every job, every open output channel — and deal with whichever comes first. The only thing `until` changes is when the loop returns, and what a key means:

| `until` | Returns when | A key | Ctrl+C |
|---|---|---|---|
| `Foreground` | job 0 is over | kept for later | ends job 0's programs |
| `Key` | a key arrives | is the answer | is the answer (the main loop abandons the line) |
| `Jobs` | no job is left | kept for later | ends the wait, not the jobs |

Reading a key at the prompt is now `attend(Until::Key)`. So while I sit and think, the shell is quietly appending a background job's output to its file. From the boot test's log — the first job finished *while the next line was being typed*:

```
[1] cat ls ls | cat > big4
[shell] ran "cat ls ls | cat > big4 &"
forever[process] 123 "cat" exited with code 0
[process] 124 "cat" exited with code 0
 sleep > flog &
```

(`forever` and ` sleep > flog &` are the keys of the next line being echoed; the two programs ending are logged in between.)

### Why this is one `object_wait`

The kernel call from entry #17, `object_wait`, takes a list of handles of any kinds and returns the first that is ready. It was built so the shell could watch a process and the keyboard together. It turns out to be exactly what makes this design possible: processes, channels and the keyboard, for any number of jobs, in one call. There is no thread per job and no polling round the list.

It also sets the limit. The kernel waits on at most 32 things at once. Each job costs one per running program plus one for its output, and a foreground pipeline may need nine. So a background job is refused if it would leave no room for the largest foreground one:

```rust
const WATCHED_MAX: usize = abi::WAIT_HANDLES_MAX as usize - (PIPELINE_MAX + 1);
```

```
too many jobs are running: end one first (jobs, kill N)
```

Refusing the *background* job, at the moment it is asked for, is the right place: the alternative is discovering later that an ordinary `ls` cannot be run.

### A job outlives its line

One detail gave the struct its `sink` field. A foreground program's output goes "where the line said" — the shell keeps the current line's redirection, and forgets it when the line is done. A background job is still writing long after its line is done, while other lines with other redirections come and go. So when a job goes to the background it takes its file's path with it. The foreground job has `sink: None`, meaning "ask the line".

## `kill`, and what the shell can end

```
taaj:/> kill 2
[2] stopped  forever sleep
```

`kill` takes a **job number**, not a process number. That is not a simplification; it is what the capability design says. To end a process you need a handle to it (entry #17), and the shell has handles to exactly the processes it started. It cannot end the file service or the display, because nobody gave it the means — there is no `kill 1`-as-root to forget to guard. `ps` shows every process; `jobs` shows the ones that are yours to end.

For a pipeline, `kill` ends every program in it.

After asking the kernel to end each program, `kill` waits for it to have ended before returning. The kernel ends a program wherever it is — computing, sleeping, waiting on a channel (all four were tested in entry #17) — so that wait is short, and it means the very next prompt can say `[2] stopped` rather than the one after.

## Saying that a job ended

A job that ends is announced **before the next prompt**, never at the moment it ends:

```rust
fn prompt(&self) {
    self.report_jobs();
    ...
```

At the moment it ends I might be in the middle of typing, or another command might be in the middle of printing; a line dropped in there helps nobody. Every Unix shell does the same. The price: a job that ends while the shell sits idle is not announced until I press Enter.

What it says is the first thing that was not a plain finish: `stopped`, `NAME was killed by the kernel`, `NAME exited with code 3` — or `done`. A writer that ended because its reader left (`NO_READER`, entry #32) counts as done.

## What cannot go to the background

```
taaj:/> echo hi &
echo is one of the shell's own commands: it cannot run in the background
```

`echo`, `cd`, `ps` and the rest are code in the shell, running now. There is nothing to not wait for. (A Unix shell runs them in a copy of itself; I have no copy to make.) Three cases do work, because a program is involved:

- `run hello &` and `with files given &` — these start a program, and the `&` is for it;
- `echo bg | wc > count &` — the shell's own command runs first, to its end (entry #32), and the `&` applies to the programs after it.

## Reading the line

`&` is one more token, text when quoted, and **allowed only as the last thing on the line**:

```
taaj:/> ls & wc
a & can only end the line
```

In a Unix shell `a & b` runs `a` in the background and then `b`, and `a && b` runs `b` only if `a` succeeded. I do neither. Quietly treating `&&` as something else would run a command the user made conditional — so anything but a final `&` is refused.

`parse` now returns a `Line`: the commands, and `background: bool`.

## `wait`

```
taaj:/> wait
```

Waits until every background job is over, or Ctrl+C (which ends the waiting, not the jobs). I added it because the boot test needed a way to say "now be sure they have finished" — and then it cost six lines, since it is only a third value of `Until`. That is the best evidence I have that the one-loop design is right: a new kind of waiting was a new enum variant.

## The design, in one table

| Decision | Chosen | Rejected, and why |
|---|---|---|
| What background means | The shell does not wait | Anything in the program or the kernel — nothing there changes |
| The foreground command | A job like the others, number 0 | A separate code path — two loops serving the same jobs |
| Serving jobs | One loop, one `object_wait` over everything | A thread per job (the shell has one thread); polling each job in turn |
| A background job's file output | Served by the shell, at the prompt and during other commands | Refusing `> file &` — the most useful form |
| `kill` names | A job | A process number — the shell holds no handle to processes it did not start |
| Announcing an ending | Before the next prompt | At once — in the middle of whatever is on the screen |
| Too many jobs | Refuse the background job | Find out when a foreground command cannot be watched |
| `a & b`, `a && b` | Refused | Guessing a meaning |
| The shell's own commands with `&` | Refused, unless they start a program | Pretending — they are over before the `&` matters |

## The bugs

None in the running system: the first boot with background jobs passed, the 50 KB arriving exact from a job nobody waited for. As with pipes, I put that down to the two entries before — every hard thing here (draining a channel before finishing, feeding without blocking, never dropping a byte) was already a tested piece, and this entry rearranged them.

The one real mistake was mine and outside the OS: a script that makes several edits to the shell's source failed on its last pattern *after* I had started the boot test in the same command — so the test ran, and passed, on the old code without the new lines typed. "Passed" was true and meant nothing. I noticed because the script's error was printed above the result. The lesson is an old one: **a test result is only about the code that was actually built.** I re-ran it and read the log to see the new lines in it before believing it.

## How I checked it

**On the host** (`make kernel64-test`): 3 new tests in `input` (29 in the crate) — a final `&` with and without a space, on a pipeline with files; every misplaced `&` including `&&`; `&` alone; a quoted `&` as text.

**In the boot test** (BIOS and UEFI), 13 new lines are typed:

| Typed | Checked |
|---|---|
| `cat ls ls \| cat > big4 &` | `[1] cat ls ls \| cat > big4` at once; later `[1] done …`; and from the host, `big4` **byte-identical** to `ls` twice |
| `forever sleep > flog &` | `[2] …`; from the host, `flog` holds the one line it printed |
| `forever yield &` | `[3] …`, and its line appears on the screen |
| `jobs` | `[2] running …` and `[3] running …` |
| `kill 3`, `kill 2` | `[3] stopped …`, `[2] stopped …` at the next prompts |
| `kill 9` | `kill: no job 9` |
| `echo bg \| wc > bgcount &` | from the host, `bgcount` is `1 line, 1 word, 3 bytes` |
| `wait`, then `jobs` | `no jobs` |
| `echo hi &`, `ls & wc` | the two refusals |
| `echo "x & y"` | `x & y` |

The check that `ps` counts the same processes, threads and channels at the start and the end of the test still passes: the killed programs are gone and nothing was leaked.

`make test64` passed.

**Not tested:**

- I have not typed any of this into a `make run64` window myself.
- **A background job still running while a foreground command runs.** The code is the same loop, but in the test the 50 KB job happened to finish during typing, before the next command started. This is the case the design exists for, and it is not exercised.
- **`wait` actually waiting.** By the time it was typed, the job had ended; it returned at once. And **Ctrl+C during `wait`**.
- **"Too many jobs"**: never reached.
- **A background job whose file cannot be written** (a full disk): it should say `[N] /file: the file system is full` when it ends.
- **A background job with `< file`**, and **Ctrl+C in the foreground while a background job runs** (it must not touch the background job).
- **`run hello &`** and **`with files given &`**.

## What is still missing

- **`fg`**: no way to bring a job back to the foreground — to wait for *one* job, or to give it Ctrl+C.
- **A background job that prints to the screen writes over whatever I am typing.** There is no terminal ownership; Unix stops such a job (`SIGTTOU`) only if asked to, so this is at least traditional.
- **A job's ending is announced only at the next prompt** — press Enter to see it.
- **Jobs die with nothing if the shell does.** Nothing adopts them; nothing ends them either.
- **`kill` cannot name one program of a pipeline**, and there is no gentler request than ending it — no "please stop" a program could act on.
- **`a && b`, `a ; b`**, and exit status in general.
- Carried over: no standard error; no keyboard as standard input; writers poll on a full channel.

That closes the shell plumbing: quoting (#30), streams and redirection (#31), pipes (#32), jobs (#33). Next on the road is the service registry.

## What I learned

- **Background is the absence of a wait** — and I only saw how much the wait had been *doing* when I tried to take it away.
- **When two loops must serve the same things, there should be one loop.** Making the foreground command an ordinary entry in the list is what removed the special case.
- **A waiting primitive that takes anything is worth more than it looks.** `object_wait` was written for "a process or a key"; it carried a job system without a change.
- **State that outlives its context must carry what it needs.** A background job cannot ask "the current line" where its output goes.
- **Authority shapes the interface.** `kill` takes a job number because a job is what the shell holds handles to.
- **A passing test proves something only about what was built.** Read the log for the thing you added.
- In Rust: an `enum` as the *mode* of a function instead of a `bool` (three cases, each named at the call site); `partition` to split a `Vec` in two by a test; `drain(..)` to take everything out of a `Vec` and keep the `Vec`; guards on match arms (`Ok((n, _)) if until == Until::Key`); `?` inside a loop that holds a `RefCell` borrow — returning drops the borrow, so nothing is left locked.

---

## Testing note

```
make run64
```

Start in the root (`cd`). `&` is Shift+`7`, `|` is Shift+`\`.

**Starting, listing, ending**

| Type | You should see |
|---|---|
| `forever sleep &` | `forever: sleep until stopped (Ctrl+C)` and `[1] forever sleep` (in either order), then the prompt at once |
| `forever yield &` | The same with `yield` and `[2]` |
| `jobs` | `[1] running  forever sleep` and `[2] running  forever yield` |
| `echo still here` | `still here` — the shell is free |
| `ps` | Two `forever` processes in the table *(not in the automated test)* |
| `kill 2` | Nothing, then `[2] stopped  forever yield` just above the next prompt |
| `kill %1` | `[1] stopped  forever sleep` — the `%` is allowed *(`kill 2` is in the automated test; the `%` form is not)* |
| `jobs` | `no jobs` |
| `kill 9` | `kill: no job 9 (see jobs)` |

**Output into a file, with nobody waiting**

| Type | You should see |
|---|---|
| `cat ls ls \| cat > big4 &` | `[1] cat ls ls \| cat > big4`, and the prompt at once |
| *(wait a few seconds, then press Enter on an empty line)* | `[1] done  cat ls ls \| cat > big4` |
| `ls ls big4` | `big4` is exactly twice the size of `ls` *(the automated test compares the bytes from the host)* |
| `forever sleep > flog &` then `kill 1` then `cat flog` | `forever: sleep until stopped (Ctrl+C)` — it went into the file, not onto the screen *(the `cat` is not automated; the host reads the file)* |

**While something else runs** *(not tested at all: please try it)*

| Type | You should see |
|---|---|
| `cat ls ls ls ls \| cat > big5 &` then at once `forever` | `forever: spin until stopped (Ctrl+C)`; the shell is held by `forever` |
| *(wait ten seconds, then Ctrl+C)* | `^C`, `forever was stopped`, and `[1] done  cat ls ls ls ls \| cat > big5` — the background job was served and finished while `forever` held the foreground, and Ctrl+C did not touch it |
| `ls ls big5` | `big5` is four times the size of `ls` |

**`wait`**

| Type | You should see |
|---|---|
| `cat ls ls \| cat > big6 &` then at once `wait` | A pause with no prompt, then `[1] done  cat ls ls \| cat > big6` and the prompt *(in the automated test the job had already ended)* |
| `forever sleep &` then `wait` then Ctrl+C | `^C` and the prompt; `jobs` still shows `[1] running  forever sleep`. Then `kill 1` *(not tested at all)* |

**The shell's own commands**

| Type | You should see |
|---|---|
| `echo hi &` | `echo is one of the shell's own commands: it cannot run in the background` |
| `echo bg \| wc > bgcount &` then `wait` then `cat bgcount` | `[1] echo bg \| wc > bgcount`, `[1] done …`, and `1 line, 1 word, 3 bytes` *(the `cat` is not automated; the host reads the file)* |
| `run hello &` | `[1] run hello`, and hello's output *(not tested at all)* |

**A `&` that is refused, or only text**

| Type | You should see |
|---|---|
| `ls & wc` | `a & can only end the line` |
| `ls && wc` | The same *(host-tested only)* |
| `&` | `there is a & with no command before it` *(host-tested only)* |
| `echo "x & y"` | `x & y` |

**`help`** — still one screen, now with `COMMAND &`, `jobs`, `kill N` and `wait`.

**From the host** — with QEMU closed:

```
tools/tfstool-rs/target/release/tfstool get build/taaj-disk.img big4 | cmp - <(cat user/target/x86_64-unknown-none/release/ls user/target/x86_64-unknown-none/release/ls) && echo same
tools/tfstool-rs/target/release/tfstool get build/taaj-disk.img flog
```

The first prints `same`; the second prints forever's one line.

**Cleaning up**, in the OS: `rm big4 big5 big6 flog bgcount`.

Automated: `make test64`.
