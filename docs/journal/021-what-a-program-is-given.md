# What a Program Is Given, and a Client That Does Not Wait Forever

*Journal entry #21 · per-program grants in the shell; a time limit on every answer*

Two debts from earlier entries, both written down at the time:

- Entry #16: *"Every program the shell starts gets both the screen and the files, even `primes`, which needs neither."*
- Entry #17: *"The shell itself still hangs if the file service stops answering."*

Both are about the same thing from two sides: how much one program's fate should depend on another. This entry pays them.

---

## Part 1: a program gets what it needs

### The problem

A capability system promises that a program can do only what it was handed. The promise is only as good as the handing-out. Since entry #16 the shell gave *every* program it started a channel to the file service — so `primes`, a program that prints fifteen numbers, could also have deleted every file on the disk. Nothing forced that; it was simply the easy thing to write.

This is the same disease as "ambient authority" (entry #9), reintroduced one level up: not by the kernel, but by a generous parent.

### Who decides?

Something has to decide which programs get the files. Three candidates:

1. **The program asks.** It cannot: asking for authority you do not have, and receiving it because you asked, is no restriction at all.
2. **A list in a file on the disk** — "these programs may use files". Tempting, and wrong: any program that can *write files* could add itself to the list. The policy would be editable by exactly the things it is supposed to restrain.
3. **The holder of the authority decides.** The shell holds the handle; the shell chooses whom to share it with.

The third is the only one consistent with the model. So the policy lives in the shell:

```rust
/// The programs that are given the file service when run by name:
/// the ones whose whole job is files. Everything else gets the screen
/// and its arguments - `with files COMMAND` adds the files, by the
/// user's own decision.
const FILE_PROGRAMS: &[&str] = &["ls", "cat", "write", "append", "rm"];
```

and one line where the handles for a new program are collected:

```rust
let files_too = with_files || (image.is_some() && FILE_PROGRAMS.contains(&name));
...
(abi::startup::FILES, Self::delegate(self.files.as_ref().filter(|_| files_too).map(Files::channel))),
```

`image.is_some()` is there for a reason: the names in the list mean *files on the disk*. A program of the same name built into the kernel image would be a different program, and should not inherit the trust.

Nothing else changed — not init, not the kernel, not the request format. The startup flags from entry #16 already let a program be started with any subset of handles; the shell just had not been choosing.

### The user's override: `with files`

A fixed list cannot know about programs written tomorrow. So there is a way for the person at the keyboard to grant the files explicitly:

```
with files COMMAND
```

The authority is the user's, exercised through the shell, per run. A program still cannot grant itself anything.

### Seeing it: `given`

A new program on the disk reports what it was started with:

```
taaj> given a b
given: the screen, no files
given: arguments "a b"
taaj> with files given
given: the screen, and the files (12 of them)
```

It is `taaj::file_service()` returning `None` in the first case — not an error from the file service, not a permission check that failed. There is nothing to ask. The file service does not exist for that process.

### Is the list a weakness?

A fair objection: if someone replaces the file `ls` with a malicious program, it gets the files, because of its name. True — but replacing `ls` requires already having the files. The list cannot be used to *gain* authority, only to keep what was already had. What it does not protect against is a user being tricked into running `with files something`. That is the user's decision, visibly made; it is the right place for the risk to sit.

What is still coarse: "the files" is all of them, read and write. Handing a program *one* file, or a read-only view, needs the file service to mint narrower handles. That is a later step, and the natural one.

## Part 2: a client that does not wait forever

### What `PeerClosed` does not cover

Since entry #13, every request to the file service carries a fresh reply channel. If the service *dies*, the reply end dies with it and the client hears `PeerClosed`. That handles a crash.

It does not handle a service that is **alive and stuck** — in a loop, or waiting on something that will never come. Its handles are all still open. The client's `receive` waits, correctly, forever. And since the shell is a client, one stuck service froze the shell, and with it the whole visible system.

### The fix

Entry #19 added `object_wait_until`: wait for a handle to be ready, but only until a given tick. On top of it, in the runtime:

```rust
pub fn receive_until(channel: Handle, buffer: &mut [u8], handles: &mut [Handle], deadline: u64) -> Result<(usize, usize), Error> {
    match object_wait_until(&[channel], deadline)? {
        Some(_) => receive(channel, buffer, handles),
        None => Err(Error::TimedOut),
    }
}
```

`object_wait_until` consumes nothing, so the `receive` after it cannot block: the channel is known to have a message or a closed peer.

The file client and the disk client now wait for each answer with a deadline — `ANSWER_WAIT`, 500 ticks, 5 seconds:

```rust
let deadline = crate::ticks() + self.patience;
let received = sent.and_then(|()| crate::receive_until(answer, reply, &mut [], deadline));
let _ = crate::close(answer);
```

and a timeout is reported in words: `the file service is not answering`.

### Why giving up is safe here

A timeout in a request-reply protocol is usually dangerous: the late reply arrives eventually and is taken for the answer to the *next* request. Everything after is off by one.

Here it cannot happen, because of the reply-channel-per-request design. Giving up **closes the client's end of that one channel**. A reply sent later finds no one; the service's `send` fails, and the service already ignores that (a client may always have died). The next request has its own new channel. A design chosen in entry #13 for crash-safety turns out to make timeouts safe too.

This is also why I did **not** add a timeout to the shell's wait for init's reply: that conversation uses one shared channel, where a late reply *would* be misread. init never waits on anything, so it has no way to get stuck in the same manner — and changing that protocol is a separate piece of work.

### Why 5 seconds

A limit must be long enough never to fire on a healthy system, and short enough to matter to a person. Every request is small — one message of at most a kilobyte, or one commit — and takes milliseconds. Five seconds is several hundred times the worst I have measured, and about as long as someone will stare at a frozen prompt before deciding it is broken.

The cost of a wrong guess is asymmetric and mild: a request that is given up on *may still be carried out* by a slow service. For a write, the client is told "not answering" while the data lands anyway. The client must treat a timeout as "unknown", not "did not happen". That is inherent in timeouts, in every system.

### One correction in the shell

The shell looks up an unknown word as a file. Its code treated *any* failure of that lookup as "no such file", so with a stuck file service every command would have answered `unknown command` — after five seconds, and misleadingly. It now tells the two apart: a refusal means no such file; a timeout is reported as what it is.

## How I checked it

- **A silent service, in `fscheck`.** The test makes a channel and keeps *both* ends — so requests go in and nobody ever reads them — and points a file client at it with a patience of 10 ticks. The call must fail with `TimedOut`, after 10 to 13 ticks: not sooner, not never. It runs twice at every boot, and the self-test still requires 0 channels left afterwards — so the abandoned request, and the reply-channel end travelling inside it, are freed.
- **Boot tests** (BIOS and UEFI): `given a b` must say `no files` and show its arguments; `with files given` must count the files; `with` alone must print its usage. `ls` now lists 12 files.
- The `ps` comparison from entry #16 still matches, and `make test64` passes.

**What is not tested:** the real thing — the actual `fs` service stuck, and the shell reporting it. There is no way yet to make a healthy service hang on demand, as F12 does for the keyboard driver's crash. The mechanism is tested; the scenario is not.

## What I learned

- **Least privilege is lost at the parent, not the kernel.** The kernel enforced everything perfectly while the shell gave everything away.
- **Policy belongs to whoever holds the authority.** Anywhere else, the policy becomes something to attack.
- **Good structure pays twice.** A reply channel per request was for crashes; it made timeouts safe for free.
- **A timeout means "unknown".** It ends the waiting, not the uncertainty.

---

## Testing note

```
make run64
```

| Type | You should see |
|---|---|
| `given` | `given: the screen, no files` |
| `given one two` | The same, then `given: arguments "one two"` |
| `with files given` | `given: the screen, and the files (N of them)` — N as `ls` reports |
| `with` or `with files` | `usage: with files COMMAND` |
| `ls`, `write t x`, `cat t`, `rm t` | All work as before — these five are given the files |
| `primes` | The primes — it never needed the files, and no longer has them |
| `help` | `given` and `with files COMMAND` in the list |
| `ps` before and after all this | The same counts |

In the terminal log, near the top: `[fscheck] … careless requests, a silent service: all passed`, twice.

The timeout cannot be triggered by hand yet (see "What is not tested"). If you ever *do* see `the file service is not answering` in normal use, that is a real finding — tell me what you had typed.

Automated: `make test64`.
