# Programs That Take Arguments and Use Files

*Journal entry #16 · `ls`, `cat`, `write`, `append` and `rm` leave the shell*

Until this step, TheTaaJ could run a program from the disk, but that program was deaf and blind: it got one channel to the screen and nothing else. It could compute, print and exit. Everything that touched files — `ls`, `cat`, `write`, `append`, `rm` — was code inside the shell.

That is backwards for the system I want. A shell should be a program that *starts other programs*, not a program that contains them. This entry is about what it took to turn that around: giving a program arguments and access to files, without giving up the rule that a process can do only what it was explicitly handed.

---

## Why this is harder here than on Unix

On Unix, a new process inherits its parent's open files, its environment and its arguments, and `ls` simply opens the current directory. There is a global file system, and every process can reach all of it (subject to permission checks made *later*, at each call).

TheTaaJ has no global anything. A process holds **handles**, and a handle is the only way to reach a kernel object. The file service is not a place; it is a process, reached through a channel. A program that was not given a handle to that channel has no files — there is nothing to look up and no name to guess.

So "let `ls` list the files" really means three separate questions:

1. **How does a program find what it was given?** It has handles numbered 1, 2, 3… — but which one is the screen and which one is the file service?
2. **Who gives a program its handles?** The shell asks for the program; `init` is the only process allowed to start one.
3. **How do arguments get in?** There is no `argv` on the stack; the kernel knows nothing about text.

## 1. Startup flags: saying what each handle is

A process starts with `n` startup handles (numbered `1..=n`) and one 64-bit *argument* from its parent, passed in a register. I made that argument a set of flags. Each flag says "one more handle is present", and the handles come **in the order of the flags' bits**:

```rust
pub mod startup {
    /// A channel to the display - print there too.
    pub const STDOUT: u64 = 1 << 0;
    /// A channel holding one message: the program's arguments.
    pub const ARGUMENTS: u64 = 1 << 1;
    /// The file service's request channel.
    pub const FILES: u64 = 1 << 2;
}
```

Finding a handle is then a count of the bits below its flag. If a program was started with `ARGUMENTS | FILES` (no screen), its arguments are handle 1 and its files handle 2; with all three, they are handles 2 and 3. The runtime library does it in one line:

```rust
fn flagged_handle(flag: u64) -> Option<Handle> {
    let flags = STARTUP_ARGUMENT.load(Ordering::Relaxed);
    (flags & flag != 0).then(|| startup_handle(u64::from((flags & (flag - 1)).count_ones()))).flatten()
}
```

`flags & (flag - 1)` keeps only the bits *below* `flag`; `count_ones()` counts them. A program never deals with this: it calls `taaj::arguments()` and `taaj::file_service()`.

**Why flags and not a fixed layout** ("handle 1 is always stdout")? Because most programs will not get everything. A fixed layout needs placeholder handles for the missing ones, and every new kind of handle shifts the numbering for every program ever written. With flags, an old program simply never asks about a flag it does not know.

## 2. Arguments: a channel with one message in it

The kernel moves two things between processes: bytes in messages, and handles. It does not know what an argument is, and I did not want to teach it. So arguments use what already exists: `init` creates a channel, sends the text as one message, **closes its own end**, and gives the program the other end, receive-only.

```rust
let made = taaj::channel_create().and_then(|(tx, rx)| {
    let sent = taaj::send(tx, run.arguments, &[]);
    let _ = taaj::close(tx);
    sent.map(|()| rx).inspect_err(|_| close(&[rx]))
});
```

A message already in the queue survives the sender closing its end, so the program finds it waiting whenever it asks. No new system call, no new kernel concept.

For now the arguments are the raw text typed after the program's name. `cat` splits it on whitespace; `write` takes the first word as the file name and the rest as the text. Quoting, and an `argv` array, belong to the libc layer later.

## 3. The interesting part: whose authority is it?

The shell cannot start programs. It has no *launcher* handle; only `init` does. So the shell sends `init` a request, and `init` starts the program.

The obvious design is: the request says "start `cat`, and give it files", and `init` — which holds a handle to everything — hands the program the file service. **That design is a security hole.** It makes `init` reach into its own, much larger authority on behalf of whoever asks. Any process that can send `init` a request could then get the file service, or the screen, whether or not it was ever given them. This mistake has a name — the *confused deputy* — and it is exactly what capabilities exist to prevent.

So `init` gives nothing of its own. Instead:

- The **shell** makes a copy of *its own* handle to the screen, and of *its own* handle to the file service, and sends those copies along with the request.
- **`init`** passes each one on to the new program, cut down to "send only", and adds only the arguments channel it made.

A program can therefore never get more than the process that asked for it already holds. If one day there is a restricted shell with no file access, the programs it starts have none either — with no extra code anywhere.

To copy and hand over a handle, the shell needs two rights on it: `DUPLICATE` and `TRANSFER`. Until now `init` gave every service handles *without* those rights, so that a handle stayed where `init` put it. Now exactly two of the shell's handles carry them — and `init` asserts that only those two do:

```rust
const DELEGABLE: u32 = DUPLICATE | TRANSFER;
...
Grant { handle: display_tx, rights: SEND | DELEGABLE }, // 7: text to the screen
Grant { handle: files_out, rights: SEND | DELEGABLE }, // 11: requests, to fs
...
debug_assert!(grants.iter().enumerate().all(|(n, g)| g.rights & DELEGABLE == 0 || matches!(n, 7 | 11)));
```

### The request

The old request was the text `"run NAME"`. The new one is a few bytes (`abi::init::Run`):

```
[kind] [flags] [name length] [name] [arguments]
```

`kind` is `BUILT_IN` (a program inside the kernel image) or `IMAGE` (the first handle is a memory object holding the program's file, which the shell read from the disk). `flags` are `STDOUT` and `FILES`: for each one set, one more handle follows.

`init` checks that the number of handles is exactly one for the image plus one per flag — no more, no fewer — and that the flags are ones it knows. Anything else gets `error: unknown request`, and every handle that came with it is closed. `Run::decode` is tested on the host with truncated, mislabelled and non-text requests: it never panics and never returns a name that is not there.

### A trap in the error handling

After `init` starts a program, the handles it passed on are gone from its table — they belong to the new process. My first draft closed "all the handles that came with the request" at the end, as the old code did. That would have been a real bug: when a handle leaves the table its *number* becomes free, and the very next handle created — the handle to the new process — can be given that same number. Closing the stale number would have closed the process handle `init` was about to send back. The code now closes only what it still owns: the image always, and the passed handles only if the start failed.

## 4. The shell: any file can be a command

With that in place the shell needed one new rule: **a word that is not one of its own commands is looked up as a file.** If a file of that name exists, the shell reads it into a memory object and asks `init` to run it with the rest of the line as arguments.

- A program run by its name alone is a *command*: the shell says nothing when it succeeds, and reports the exit code only if it is not 0.
- `run NAME ARGS` is still there. It always says how the program ended, and it is the only way to reach the two programs built into the kernel image (`hello`, `peek`).

Then the five file commands came out of the shell and became small programs. `ls` in full:

```rust
fn main() -> i64 {
    let Some(files) = taaj::file_service() else {
        println!("ls: started without the file service");
        return 2;
    };
    let mut bytes = 0;
    let listed = files.list(|name, size| {
        println!("{size:>8}  {name}");
        bytes += size;
    });
    match listed {
        Ok(0) => println!("no files"),
        Ok(count) => println!("{count} file{}, {bytes} bytes", if count == 1 { "" } else { "s" }),
        Err(error) => {
            println!("ls: {}", error.describe());
            return 1;
        }
    }
    0
}
```

`cat` and `rm` take several names and exit with 1 if any of them failed. `write` and `append` differ only in where the text goes, so they share one source file (`user/src/bin/common/store.rs`) — a program has no way yet to learn its own name, so they cannot be one binary.

The shell itself no longer changes any file. It only reads the ones it runs.

## What this costs

Honest accounting:

- **With no disk, or a disk without these programs, there are no file commands.** They answer `unknown command`. `make run64` now attaches a persistent disk image (`build/taaj-disk.img`) and installs the programs on it at every build, so normal use is unaffected.
- **Every program the shell starts gets both the screen and the files**, even `primes`, which needs neither. Choosing per program what to hand down is a later step — the mechanism allows it, the shell just does not use it yet.
- **Running `ls` now means loading a 26 KB program from the disk.** That turned out to be slow, which entry #18 fixes.

## How I checked it

- **Host tests** for the request format: encode/decode round trip, requests that do not fit, and seven malformed requests.
- **Boot tests** (BIOS and UEFI): `cat notes`, `ls`, `cat saved welcome` (two names), `run cat` (usage, exit code 2), a `cat` of a missing file (exit code 1), an `rm` where both names are missing, a `write` with no text.
- **A leak check.** The test types `ps` before the first program runs and again after the last. Both must print the same line — `6 processes, 7 threads, 7 channels` — or a handle was forgotten somewhere along shell → init → program. They match.
- **The disk itself.** After QEMU exits, the host tool reads the file the OS wrote (`saved` must hold `kept`) and confirms the removed file is gone.

---

## Testing note

Run the system with its persistent disk:

```
make run64
```

A QEMU window opens; the kernel's log appears in the terminal. Type in the QEMU window.

| Type | You should see |
|---|---|
| `help` | The command list, with `ls`, `cat`, `write`, `append`, `rm` under "On the disk" |
| `ls` | `ls`, `cat`, `write`, `append`, `rm`, `primes`, `forever` with their sizes, then `N files, B bytes` |
| `write notes hello` | `notes: 6 bytes` |
| `append notes world` | `notes: 12 bytes` |
| `cat notes` | `hello` then `world` |
| `cat notes nothing` | The two lines, then `cat: nothing: no such file`, then `cat exited with code 1` |
| `rm notes` | Nothing (success is silent) |
| `cat notes` | `cat: notes: no such file` and `cat exited with code 1` |
| `write` | `usage: write FILE TEXT` and `write exited with code 2` |
| `run primes` | The primes below 50, then `primes exited with code 0` |
| `primes` | The primes only — no exit line |
| `nosuchthing` | `unknown command: nosuchthing (try help)` |
| `ps` | Only the six services — no `ls` or `cat` left running |

Persistence: `write keep me`, close QEMU, `make run64` again, `cat keep` — it should print `me`.

In the terminal log, every program start appears as `[init] started NAME for the shell, from a file`.

The automated version of all this is `make boot64-test` (BIOS) and `make boot64-uefi-test` (UEFI).
