# The Current Directory: a Place to Stand That the File Service Never Hears About

*Journal entry #28 · `cd`, `pwd`, relative paths, `.` and `..` — all of it worked out by the client, with the file service left exactly as it was*

Entry #26 gave the disk a tree. It did not give me anywhere to stand in it. Every name was a path from the root, so working in `docs/notes` meant typing `docs/notes/` in front of everything, every time. A tree nobody can stand in is only a longer way of spelling file names.

This entry adds the place to stand: `cd`, `pwd`, a prompt that says where I am, names that mean "in this directory", and `.` and `..`. The interesting part is not the commands — they are a few lines each. It is **where the current directory lives**, because in TheTaaJ the usual answer is not available.

---

## Where Unix keeps it, and why I cannot

In Unix the current directory belongs to the **kernel**. Every process has one; `chdir()` is a system call; a child inherits its parent's at `fork`. When a program opens `notes`, the kernel starts the lookup from that process's directory.

That works because in Unix the kernel is the file system's front door: every `open` goes through it, so it is the natural place to hold "where this process is".

TheTaaJ's kernel does not know what a file is. Files are a conversation between a program and the `fs` service over a channel; the kernel carries the messages and understands none of them. So there are three places the current directory could live:

| Where | What it would mean | Why not / why |
|---|---|---|
| **The kernel** | A per-process string the kernel stores and hands out | The kernel would be keeping a fact about a service it does not know exists. Mechanism in the kernel, policy outside — this is policy. |
| **The file service** | `fs` remembers, per client, where that client is | `fs` would hold state about its clients. It is restartable (entries #23–#25) precisely because it holds none: when init replaces a hung `fs`, every client's directory would be silently reset to the root. And "per client" needs `fs` to tell clients apart, which the shared request channel does not let it do. |
| **The client** | Each program knows its own directory and sends whole paths | Nothing in the kernel, nothing in the service. The service's protocol does not change by one byte. |

I wrote in entry #26 that the third was the plan — "state about a client belongs to the client". This is that plan carried out.

So the rule is: **the file service still takes only whole paths from the root. Turning `../plan` into `docs/plan` happens before the request is sent.**

## What it is

Three small pieces:

1. **A function**, `resolve(directory, path)`, that turns a path as typed into a path from the root. Pure text; 12 lines.
2. **The shell** remembers one string — its current directory — and has `cd` and `pwd`.
3. **A program is told the directory it was started in**, the same way it is told its arguments, and the file library resolves every name against it.

## Piece 1: `resolve`

It lives in `fileserver/src/path.rs`, next to the protocol, because both the shell and every program need it and that crate is already shared by all of them (and is tested on the host).

```rust
pub fn resolve(directory: &str, path: &str) -> String {
    let from = if path.starts_with('/') { "" } else { directory };
    let mut names: Vec<&str> = Vec::new();
    for name in from.split('/').chain(path.split('/')) {
        match name {
            "" | "." => {}
            ".." => {
                names.pop(); // at the root: nothing to pop, and no error
            }
            name => names.push(name),
        }
    }
    names.join("/")
}
```

The whole idea is a **stack of names**. Walk the names left to right; an ordinary name is pushed; `..` pops the last one; `.` and empty names do nothing. What is left on the stack, joined with `/`, is the answer.

| I am in | I type | Stack as it goes | Result |
|---|---|---|---|
| `docs/notes` | `today` | docs, notes, today | `docs/notes/today` |
| `docs/notes` | `../plan` | docs, notes → docs → docs, plan | `docs/plan` |
| `docs/notes` | `/welcome` | *(starts empty)* welcome | `welcome` |
| `docs` | `./a/../b` | docs, a → docs → docs, b | `docs/b` |
| `docs` | `../../..` | docs → *(empty)* → *(empty)* → *(empty)* | `` (the root) |

Some Rust in those lines, since I am still learning it:

- **`from.split('/').chain(path.split('/'))`** — `split` gives an *iterator* over the pieces of a string; `chain` glues two iterators end to end. So the loop runs over the directory's names and then the path's names as if they were one path. Nothing is built in between; the pieces are produced one at a time as the loop asks for them.
- **`Vec<&str>`** — a list of *borrowed views* into the two input strings. Pushing a name copies a pointer and a length, not the text. The only allocation of text is the `join` at the end. (The borrow checker makes sure the views cannot outlive the strings they look into — which is why the function returns an owned `String`, not a `&str`: the result is new text that belongs to nobody else.)
- **`match name { "" | "." => …, ".." => …, name => … }`** — matching on string contents, with `|` for "either", and a last arm that catches everything else and gives it a name.

### Three decisions inside those 12 lines

**`..` at the root stays at the root.** There is nothing to pop, and `Vec::pop` on an empty list just returns `None`. Unix does the same (`cd /..` leaves you in `/`): the root is its own parent. The alternative — an error — would make `cd ../../..` fail or succeed depending on how deep I happen to be, which helps nobody.

**It is done on the text alone; the disk is not asked.** `nosuch/../welcome` resolves to `welcome` even though `nosuch` does not exist. Unix does *not* do this: it walks the real directories one at a time, so `nosuch/..` fails. Unix has to, because of **symbolic links** — if `a` is a link to `/x/y`, then `a/..` is `/x`, not the directory `a` sits in, and only the disk knows that. TheTaaJ has no links. Without them the text says everything there is to say, and resolving it costs no requests at all. (If links ever arrive, this function is the thing that has to change, and I have written that at the top of the file.)

**The directory is resolved too.** `resolve("/docs//notes/", "x")` gives `docs/notes/x`. A program gets its directory from whoever started it; tidying it by the same loop means nothing odd in it survives.

### The service still refuses `..`

I did not relax the file service. A request containing `.` or `..` is still answered `not a valid name`, on both stores, and the host tests for that are unchanged. A correct client never sends one. A careless or hostile one that does gets a refusal, not a walk up the tree. The file system never having to *interpret* `..` is one less thing in it that can be wrong.

## Piece 2: the shell

The shell gained one field:

```rust
/// The current directory: a resolved path from the root, "" for the
/// root. Only a name - whether it is still there is found out when
/// it is used.
directory: RefCell<String>,
```

`RefCell` deserves a sentence. The shell's `run` takes `&self` — a shared, read-only borrow — because it calls itself (`time ls` runs `ls` through the same function). Rust normally forbids changing anything through a shared borrow. `RefCell` moves that check from compile time to run time: `borrow_mut()` hands out the one writer, and panics if somebody else is reading at that moment. It is the standard tool for "this struct is shared, but this one field must change". The price is that I have to keep borrows short — see the bug below that I *avoided* by cloning.

`cd` is then:

```rust
fn cd(&self, argument: &str) {
    let wanted = path::resolve(&self.directory.borrow(), argument);
    let Some(files) = self.look.as_ref().or(self.files.as_ref()) else {
        return self.write(b"cd: no file service\n");
    };
    match files.stat(&wanted) {
        Ok((Kind::Directory, _)) => *self.directory.borrow_mut() = wanted,
        Ok((Kind::File, _)) => say!(self, "cd: {argument}: not a directory\n"),
        Err(error) => say!(self, "cd: {argument}: {}\n", error.describe()),
    }
}
```

Work out the new path from the text; ask the file service one question — *is that a directory?* — over the shell's **read-only** channel; if yes, remember it. `cd` with nothing after it goes to the root (there is no home directory yet). `pwd` prints the string with a `/` in front. The prompt does the same: `taaj:/docs> `.

Why `cd` must be a built-in and can never be a program: a program is another process. It could change *its own* idea of the directory and then exit, and the shell's would be untouched. This is true in Unix too, for the same reason, and it is the classic question about why `cd` is not in `/bin`.

### Which `ls` is `ls`?

Until now, a command that was not built in was looked up as a file by that name — from the root, because every name was from the root. With a current directory there is a choice to make: after `cd docs`, what does typing `ls` run?

- If names are resolved against the current directory like everything else, `ls` becomes `docs/ls`, which does not exist. Every command stops working the moment I leave the root.
- If the current directory is searched first and the root second, then a file called `ls` in whatever directory I happen to be standing in *replaces* the real one. That is an old and well-known trap (it is why `.` is not in `PATH` on any sane Unix system).

So the rule is the Unix one, for the Unix reason:

```rust
let name = if name.contains('/') {
    resolved = path::resolve(&self.directory.borrow(), name);
    resolved.as_str()
} else {
    name
};
```

- **A bare name is a command**, and commands are the files in the root, wherever the shell is.
- **A name with a `/` in it is a path**, resolved like any other: `tools/given`, `/tools/given`, and — to run something in the directory I am in — `./given`.

(The `let resolved;` with no value, assigned inside one branch, is a Rust idiom for "this value must live as long as `name` does, but only exists on one path". Declaring it outside the `if` gives it the outer scope's lifetime, so the `&str` borrowed from it stays valid afterwards.)

"The root" is standing in for a proper programs directory. When there is a `/bin`, this is the one line that changes.

The permission rule from entry #21 is untouched, and slightly firmer than before: the files are granted by *bare* name (`ls`, `cat`, `write`, …), and a bare name now always means the program in the root. `./rm` in some directory is a path, is not on the list, and gets the screen only.

## Piece 3: telling a program where it was started

`cat today` typed in `docs/notes` runs the program `cat` with the argument `today`. `cat` is a separate process; it has to learn that `today` means `docs/notes/today`. Two ways:

- **The shell rewrites the arguments** before starting the program. It cannot: the shell does not know which words are file names. In `write notes hello`, `notes` is a file and `hello` is text; only `write` knows that.
- **The program is told the directory**, and resolves its own names. This is what Unix does (the child inherits the directory), and it is the only one that works.

A program already receives its arguments as a message waiting in a channel (entry #16). The directory now travels the same way. The request from the shell to init grew one field:

```
before:  [kind] [flags] [name length] [name] [arguments]
now:     [kind] [flags] [name length] [name] [directory length] [directory] [arguments]
```

and there is one more startup flag in `abi`:

```rust
/// A channel holding one message: the directory the program was
/// started in, as a path from the root - what its relative paths
/// start from. Absent means the root. It is a name and nothing
/// more: it lets the program reach no file it could not reach
/// without it.
pub const DIRECTORY: u64 = 1 << 3;
```

init does for the directory exactly what it did for the arguments — makes a channel, puts the one message in, gives the program the receiving end — and I folded the two into one closure:

```rust
let waiting = |text: &[u8]| -> Result<Option<Handle>, Error> {
    if text.is_empty() {
        return Ok(None);
    }
    let (tx, rx) = taaj::channel_create()?;
    let sent = taaj::send(tx, text, &[]);
    let _ = taaj::close(tx);
    sent.map(|()| Some(rx)).inspect_err(|_| close(&[rx]))
};
```

`Result<Option<Handle>, Error>` reads as three outcomes: *nothing to send* (`Ok(None)`), *a channel with the text waiting in it* (`Ok(Some(rx))`), or *it could not be made* (`Err`). An empty directory — the root — makes no channel at all, so a program started in the root is started byte-for-byte as it was before this entry.

On the program's side nothing changed in any program's source. `taaj::file_service()` reads the directory message and builds the file client *in* that directory:

```rust
pub fn in_directory(requests: Handle, directory: &str) -> Files {
    Files { directory: path::resolve("", directory), ..Files::new(requests) }
}
```

(`..Files::new(requests)` is *struct update syntax*: "every field I did not name, take from this other value".) And every call resolves first:

```rust
fn path(&self, name: &str) -> String {
    path::resolve(&self.directory, name)
}

pub fn remove(&self, name: &str) -> Result<(), FileError> {
    let mut reply = [0u8; MESSAGE_MAX];
    self.call(Request::new(protocol::REMOVE, &self.path(name), 0), &mut reply).map(|_| ())
}
```

So `cat`, `write`, `append`, `rm` and `mkdir` understand `today`, `../plan` and `/welcome` without one line of them changing. `ls` changed by one word: with no argument it now lists `"."` instead of `""`, so that when the directory is gone the message has something to name. `given` gained a line saying where it was started.

```
   shell                         init                      cat
   ─────                         ────                      ───
   directory = "docs/notes"
   line: "cat ../plan"
        │
        │  Run{name:"cat", directory:"docs/notes", arguments:"../plan"}
        ├───────────────────────────►│
        │                            │ channel ◄ "docs/notes"   (DIRECTORY)
        │                            │ channel ◄ "../plan"      (ARGUMENTS)
        │                            ├─────────── spawn ────────►│
        │                                                        │ resolve("docs/notes", "../plan")
        │                                                        │   = "docs/plan"
        │                                         fs ◄── READ "docs/plan" ──┤
```

### Who is told

A program is told the directory **only if it is given the files**. `primes` or `forever` gets the screen and its arguments, as before, and has no use for a path. It is a small thing — a directory's name is hardly a secret — but the habit from entry #21 is that a program gets what its job needs, and I would rather keep the habit than argue each case.

### A name, not a key

The line in the `abi` comment matters: *it lets the program reach no file it could not reach without it.* The directory is text. A program holding the file service's channel can already name any path on the disk; being told "you were started in `docs`" adds nothing to what it can do, and a program that lies to itself about its directory hurts only itself.

There is a stronger design, and I did not build it: hand the program a **handle to a directory**, and make *that* the only thing it can reach — so `cat` started in `docs` could not open `/secrets` at all. Then the current directory would be a real capability, and `..` out of it could be refused. That needs the file service to give out per-directory channels and keep track of them, which is a larger change to `fs` and runs against "the service remembers nothing". It is the right question for the step where permissions get a real answer (the service registry, later in this era). For now: the directory is a convenience, and authority is still the channel.

## The directory can vanish under me

The shell holds a *name*. It does not hold the directory. So this is possible:

```
taaj:/tools> mkdir gone
taaj:/tools> cd gone
taaj:/tools/gone> rm ../gone
taaj:/tools/gone> ls
ls: .: no such file or directory
```

In Unix, `rm`-ing the directory a shell stands in leaves the shell holding an *inode* that still exists, nameless, until the shell leaves. TaajFS has no such thing as an object that outlives its name, and a stateless service has nobody to keep it alive for. The honest behaviour is the one above: the name is found to be gone when it is next used, the message says so, and `cd ..` or `cd` gets me out. I put exactly this sequence in the boot test so that it stays honest.

(`rm .` is not special-cased either: it means "remove the directory I am in", and works if it is empty.)

## The bugs

**A test that was right about the old system.** The first boot with the new code stopped in the kernel's self-test:

```
*** KERNEL PANIC: panicked at src/selftest.rs:344:9:
assertion `left == right` failed: fscheck failed (the code is the line of the failed check)
  left: Some(109)
```

`fscheck` exits with the line number of the check that failed, so this took seconds to find. Line 109 was:

```rust
check!(files.write("a/../b", b"x") == refused(Status::BadName), "a path that climbs");
```

That check said: *a path with `..` in it is refused*. It was true when the client passed names through untouched. Now the client resolves `a/../b` to `b` and the write **succeeds**. Nothing was broken — the test was checking the service's refusal *through a client that no longer lets `..` reach the service*. The service's own refusal is still tested where a raw request can be made (the host tests of `fileserver` and `taajfs`); in `fscheck` the line became a check that a climbing path is resolved and then looked up. It is a useful kind of failure: the test did its job by telling me a behaviour had changed, and I had to decide whether the change was the bug. Here it was the feature.

**The help text no longer fit the screen.** The console is 33 rows. `help` was 32 lines plus the prompt — it fit exactly, by luck I had not noticed. Adding four lines about `cd`, `pwd` and paths would have scrolled the top of the list off before anyone could read it. I found this by counting, not by seeing it (see "not tested" below). I merged lines until it was 32 again. The real fix is a pager, or `help COMMAND`; noted below.

**One I avoided: a borrow held across a running program.** My first sketch passed `&self.directory.borrow()` straight into the request to init. That borrow would then have been alive for the whole time the program ran — and `time cd docs` (which runs `cd` *inside* `run`) or any future command that changes directory while something else holds the borrow would panic with "already borrowed". The shell now clones the string before starting a program:

```rust
// A copy: the borrow of the directory must not last while the
// program runs.
let directory = if files.is_some() { self.directory.borrow().clone() } else { String::new() };
```

This is the `RefCell` price mentioned above: the compiler no longer checks it for me, so I check it by keeping every borrow to one expression.

## Design decisions, in one place

| Decision | Chosen | Rejected, and why |
|---|---|---|
| Where the current directory lives | In the client (the shell; each program) | The kernel (knows nothing of files); the file service (would have per-client state, lost on restart) |
| How `..` is resolved | On the text, by a stack of names | By asking the disk at each step — needed only with symbolic links, and costs a request per name |
| `..` at the root | Stays at the root | An error — makes the result depend on depth |
| Does the service accept `..` now? | No, still refused | Accepting it would put a second, different resolver in the service |
| How a program learns the directory | A startup message, like the arguments | The shell rewriting arguments (it cannot know which are paths); a kernel field |
| Who is told | Programs that get the files | Everyone — harmless, but more than they need |
| A bare command name | The file in the root, always | The current directory first — lets a file there replace `ls` |
| The directory as authority | It is not: a name only | A directory handle that confines the program — better, bigger, later |

## How I checked it

**On the host** (`make kernel64-test`, part of `make test64`):

- `fileserver`: 10 new tests of `resolve` — plain names, a leading `/`, `.`, `..`, the root as its own parent, names that only *look* like dots (`...`, `.hidden`, `a..b`), an untidy directory, that resolving twice changes nothing, and that nothing `resolve` returns contains a `.` or `..` for the service to refuse. 27 tests in the crate in all.
- `abi`: the run request with a directory round-trips; a directory and empty arguments are not confused; a request from before this change (no directory length) decodes to nothing rather than to something wrong; a directory length that claims more bytes than there are is refused.

**In the OS, at every boot:** `fscheck` — the self-test the kernel runs against a fresh file service, twice — gained a section of 14 checks that opens a second client *in* `d/inner` and names the same files as someone standing there would: `deep`, `./deep`, `../log`, `/d/log`, `../..`, four levels above the root; writes, appends, makes and removes by relative names, and lists `.` and `""`.

**In the boot test** (BIOS and UEFI), 24 new lines are typed; the testing note below is that sequence. After QEMU exits, the host tool reads the disk image and must find:

- `docs/here` containing `inside` and `more` — written by a bare name while in `/docs`, appended to from `/docs/deep` by its whole path;
- `rooted` and `home` in the **root** — written by bare names after `cd ../../..` and after a bare `cd`, which is the proof both really went to the root;
- no `tools/gone`.

The existing check that `ps` shows the same number of processes, threads and channels at the start and end of the test still passes, so the extra channel init makes for the directory is not leaked.

`make test64` — host tests, BIOS boot, UEFI boot, seven fault boots — passed.

**Not tested:**

- I have not typed any of this into a `make run64` window myself. Most of the testing note is taken from the serial log of the automated boot. A few of its lines are **not** in the automated sequence and come from reading the code instead: `cat ../../docs/./here`, `ls ..`, `ls /`, `time cd docs`, `./nothing`, `cd ../../../docs`, and running `tools/given` from the root after a `cd /`. If one of those differs from the note, the note may be what is wrong.
- The **prompt** (`taaj:/docs> `) is written to the screen only, not to the log, and the boot test's screen check counts ink rather than reading text. So the prompt's exact look has not been verified by anything but my reading of one screenshot. It is the first thing to look at when testing by hand.
- That `help` fits the screen is by counting lines (32 and a prompt on 33 rows), not by looking.
- A path that resolves to more than 200 bytes (the protocol's limit) should be answered `not a valid name`; I have not driven that case in the OS.

## What is still missing

- **No `mv` and no `cp`.** There is no rename in TaajFS at all yet; that is the next step.
- **No home directory**, so a bare `cd` goes to the root, and there is no `cd -` (back to where I was).
- **Commands are the files in the root.** There is no `/bin` and no search path.
- **The directory is not authority.** A program told it is in `docs` can still name `/anything`. A directory handle would fix that; see above.
- **A removed directory is noticed only when used**, and the shell stays "in" it until I leave.
- **`help` is at the limit of the screen.** It needs a pager or per-command help.
- **Permission by program name** is still the rule for who gets the files.

## What I learned

- **Where a piece of state lives is a design decision, not a detail.** The same feature — a current directory — sits in the kernel in Unix and in the client here, and the reason is the same in both: put it where the thing that *uses* it already is.
- **A stateless service stays stateless only if every convenience is built outside it.** `cd` was the first real temptation to let `fs` remember something. Not giving in cost twelve lines of string handling.
- **`..` is easy exactly as long as there are no links.** Path resolution on text alone is correct here and wrong in Unix, and the difference is one feature I do not have.
- **Why `cd` is always a built-in**, and why `.` is not searched for commands — two pieces of Unix folklore that I now have the reasons for, because I had to make both choices myself.
- **A failing test is a question, not a verdict.** `fscheck` line 109 was correct about the old behaviour. Reading it was how I confirmed the new behaviour was the one I meant.
- **A name is not the thing.** The shell holds the word `tools/gone`, not the directory; the word can stop meaning anything.
- In Rust: `RefCell` for a field that must change behind `&self`, and the discipline of short borrows that comes with it; `Iterator::chain`; matching on `&str`; `Result<Option<T>, E>` for "nothing / something / failed"; struct update syntax; and declaring a `let` early so a borrow from it lives long enough.

---

## Testing note

```
make run64
```

The prompt now shows the directory: `taaj:/> ` in the root. **Please look at the prompt on each line — it is the one thing no automated test reads.**

If your disk has no `docs` directory yet, make the pieces first: `mkdir docs`, `mkdir docs/deep`, `mkdir tools`, `write docs/readme hello`. (Your file sizes in `ls` will then differ from the ones below; the names should not.)

**Moving about**

| Type | You should see |
|---|---|
| `pwd` | `/` |
| `cd docs` | Nothing printed; the prompt becomes `taaj:/docs> ` |
| `pwd` | `/docs` |
| `write here inside` | `here: 7 bytes` |
| `ls` | The contents of `docs` — `     dir  deep/`, `       7  here`, and `readme` — not the root's |
| `cd deep` | Prompt `taaj:/docs/deep> ` |
| `pwd` | `/docs/deep` |
| `cat ../here` | `inside` |
| `append /docs/here more` | `/docs/here: 12 bytes` |
| `cat ../../docs/./here` | `inside` then `more` |
| `ls ..` | The contents of `docs` again |
| `ls /` | The root |
| `cd ..` | Prompt `taaj:/docs> ` |
| `cd` | Prompt `taaj:/> ` |
| `cat docs/here` | `inside` then `more` — the file is where the paths said |

**Commands and programs**

| Type | You should see |
|---|---|
| `cd tools` then `ls` | `ls` still works (it is the program in the root); lists `tools` |
| `./given` (needs `tools/given` on the disk — see the host commands in entry #26's note) | `given: the screen, no files` |
| `with files given` | `given: the screen, and the files (N of them), to read and to change` and then `given: started in /tools` |
| `given` | The same program from the root: `given: the screen, no files` |
| `cd /` then `tools/given` | `given: the screen, no files` |
| `time cd docs` | `took 0.0N s`, and the prompt is `taaj:/docs> ` |

**What is refused**

| Type | You should see |
|---|---|
| `cd /welcome` (any file) | `cd: /welcome: not a directory` |
| `cd nowhere` | `cd: nowhere: no such file or directory` |
| `cd a b` | `usage: cd [DIR]` |
| `cd /` then `cd ..` then `pwd` | `/` — the root is its own parent |
| `cd ../../../docs` then `pwd` | `/docs` |
| `./nothing` | `unknown command: ./nothing (try help)` |

**Standing in a directory that is removed**

| Type | You should see |
|---|---|
| `cd /tools`, `mkdir gone`, `cd gone` | Prompt `taaj:/tools/gone> ` |
| `rm ../gone` | Nothing |
| `ls` | `ls: .: no such file or directory` then `ls exited with code 1` |
| `cd ..` | Prompt `taaj:/tools> ` |

**`help`** — type `help`: the whole list, starting with `commands:`, should be on the screen at once with the prompt below it, and include `cd [DIR], pwd`.

**From the host** — with QEMU closed:

```
tools/tfstool-rs/target/release/tfstool get build/taaj-disk.img docs/here
tools/tfstool-rs/target/release/tfstool ls  build/taaj-disk.img tools
```

The first should print `inside` and `more`; the second should show no `gone`.

Automated: `make test64`.
