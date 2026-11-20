# The Service Registry: Asking for a Service by Name, on a Channel That Decides What Names Exist

*Journal entry #34 · a new service that knows where the others are; each client has its own channel to it, and its own list of the names that exist; the shell and the file service stop counting startup handles*

The shell plumbing is done (entries #30–#33), and this is the next thing on the road: the piece that has been missing since the era-1 plan said "the Rust kernel boots and runs the registry and init".

Here is how the shell found its services until today:

```rust
screen: taaj::startup_handle(1),
init:   taaj::startup_handle(2),
system: taaj::startup_handle(3),
files:  taaj::startup_handle(4).map(Files::new),
disk:   taaj::startup_handle(5).map(Disk::new),
look:   taaj::startup_handle(6).map(Files::new),
```

and here is the other half of that agreement, in init:

```rust
Service { name: "shell", grants: &[4, 7, 8, 9, 11, 19, 21], ... }
```

"The fifth handle is the files" is a fact held in two source files and checked by nothing. Swap two numbers in init and the shell sends its text to the file service. Now:

```rust
screen: service("display"),
files:  service("files").map(Files::new),
```

```
taaj:/> services
keys display init system files files.look disk.look
taaj:/> services disk
disk: no such service - for the shell
```

---

## The idea, and the danger in it

A registry is a table: name → where that service is. A program asks for `"files"` and gets a channel to the file service. Every microkernel system has one (it is called a name server, a service manager, a broker).

And done the obvious way, it throws away everything the last twenty entries built. This system's security rests on one sentence: **a process can only use what it holds a handle to.** The shell cannot write to the disk's sectors because it was never given the channel that allows it — not because some code checks who is asking. If there is one global table and anyone may ask it for anything, then the name `"disk"` is all it takes to reach the disk, and the registry has become the thing capabilities exist to avoid: *ambient authority* — power you have just by being there and knowing a word.

So the question for this entry was never "how do I store names". It was: **how can a program ask for services by name without names becoming power?**

## Namespaces: the channel decides what names exist

The answer uses the mechanism the system already has. There is not one channel to the registry. **Every client has its own**, and on each channel a different set of names exists:

```
                       ┌──────────── registry ────────────┐
   init ══ admin ═════►│  services:  keys display init    │
                       │             system files          │
                       │             files.look disk       │
                       │             disk.look             │
                       │                                   │
   shell ═════════════►│  "shell":  keys display init      │
                       │            system files           │
                       │            files.look disk.look   │
                       │                                   │
   fs ════════════════►│  "fs":     disk                   │
                       └───────────────────────────────────┘
```

The shell's channel has seven names on it. The file service's has one. When the shell asks for `"disk"` — which *is* registered — the answer is the same as for a name nobody ever heard of:

```
[registry] shell asked for "disk": it is not in its namespace
```

```
disk: no such service - for the shell
```

I call the list of names on a channel its **namespace**. The channel is the capability, exactly as the file service's read-only channel is (entry #21): what you can ask for was decided by whoever handed you the channel, and there is nothing to check but where the request arrived.

### "Not found", not "not allowed"

A lookup outside the namespace is answered `NOT_FOUND` — the same byte as for a name that does not exist at all. The registry's *log* says which it was; the client is not told.

This is deliberate. "Not allowed" tells the asker that the thing exists and that someone decided against them — which is an invitation, and a leak. In a namespace design there is no "allowed": for the shell, `"disk"` is not a forbidden name, it is not a name. The data structure says the same thing the reply does.

## Two kinds of thing a service is given

Not everything moved to names, and working out where the line is was the useful part.

| | Example | How it arrives | Why |
|---|---|---|---|
| What the service **works on** | kbd's interrupt line and ports; display's framebuffer; fs's own request channel | Startup handles, in a fixed order, from init | They are its own and nobody else's. There is nothing to look up — no other process should ever have them |
| What it **uses of another service** | fs uses the disk driver; the shell uses nearly everything | Asked for by name, from the registry | This is the wiring between services, which is what a registry is for |

So kbd, display and blk are untouched: they use no other service. The shell now gets **one** startup handle instead of seven, and fs gets its two request channels and the registry instead of its two request channels and the disk.

## The protocol

Two kinds of channel reach the registry.

**The admin channel**, held by init alone:

```
[REGISTER] [rights: u32] [name]        + the service's handle
[NAMESPACE] [label] 0 [name] 0 ...     + the receiving end of a client's channel
```

**A client channel**, one per namespace:

```
[LOOKUP] [name]   + a reply channel     →  [OK] + a handle,  or  [NOT_FOUND]
[LIST]            + a reply channel     →  [OK] [name] 0 [name] 0 ...
```

The reply comes on a fresh channel sent with the request, as with the file service — a client that gives up closes its end and a late answer falls on the floor.

`rights` in a registration are the rights a *client's copy* gets. The registry holds its own copy with enough rights to make more; a lookup makes one copy for that client, with only the rights init named.

Names are deliberately dull: 1 to 32 of `a-z`, `0-9`, `.` and `-`. They are compared byte for byte and shown to the user, so no two should be able to look alike.

## The crate, and the program

The decisions live in a new crate, `registry/`, with no system calls in it:

```rust
pub struct Table<H> { services: Vec<Service<H>>, namespaces: Vec<Namespace<H>> }

pub fn register(&mut self, name: &str, handle: H, rights: u32) -> Result<Option<H>, H>
pub fn add_namespace(&mut self, label: &str, names: &[&str], channel: H) -> Result<Option<H>, H>
pub fn lookup(&self, index: usize, name: &str) -> Result<(&H, u32), Miss>
```

`H` is "whatever a handle is". In the program it is a kernel handle; in the host tests it is a `u32`. This is the first time I have used a generic type for testability rather than reuse, and it worked better than I expected: the whole policy — who finds what — runs on my laptop with numbers.

Look at those return types. `register` gives back `Ok(Some(old))` when a name is replaced, and `Err(handle)` when it is refused. **The table never closes a handle** — it cannot, it does not know what one is — so anything it stops holding, or declines to take, comes back out through the return value, and the type forces the caller to do something with it. A handle silently dropped is a channel that never looks closed to its other end. Making "what happens to the handle" part of the signature means the compiler asks the question for me.

A small Rust lesson along the way: `#[derive(Default)]` on `Table<H>` would have required `H: Default`, though an empty table contains no `H` at all. The derive adds the bound to every type parameter without looking. Writing the three-line `impl<H> Default` by hand removes it.

The program around the table (`user/src/bin/registry.rs`, about 170 lines) is a loop:

```rust
loop {
    // What init has said comes first, all of it.
    while let Ok(Some(_)) = taaj::poll(&[init]) { ... admin(&mut table, ...) }
    let watched: Vec<Handle> = core::iter::once(init).chain(table.channels().copied()).collect();
    let ready = taaj::object_wait(&watched)?;
    ...serve one request from that client...
}
```

It holds a handle to every service, so it is as trusted as init. That is the reason it is small and the reason the logic is in a crate that can be tested without booting anything.

### An ordering that matters

**init's messages are drained before any client is answered**, and **init sends every service before any namespace.** Together: by the time the registry knows a client's channel exists, it already knows everything that client may find. Without this, a freshly started registry could answer the shell's first lookup from a half-filled table — "not found" for a service that was two messages away from being registered. The shell would start with no screen, about one boot in a hundred.

I did not find that by testing. I found it by asking, of the loop, "what if the client's request and init's message are both waiting?" — which is the question entry #31 taught me to ask (there it was "the process ended" and "its last output arrived").

## init: the policy in one place

```rust
let registered: [(&str, usize); 8] = [
    ("keys", 4), ("display", 7), ("init", 8), ("system", 9),
    ("files", 11), ("files.look", 21), ("disk", 16), ("disk.look", 19),
];
let namespaces: [(&str, Handle, &[&str]); 2] = [
    ("shell", shell_names_rx, &["keys", "display", "init", "system", "files", "files.look", "disk.look"]),
    ("fs", fs_names_rx, &["disk"]),
];
```

That second table is the system's access policy between services, and it can be read. Before, the same information was a row of numbers — `&[4, 7, 8, 9, 11, 19, 21]` — that had to be decoded against a list of twenty-five grants.

Note `disk` and `disk.look`. They are two channels to the same driver: one takes reads and writes, one reads only. fs can find the first; the shell can find the second. The registry does not know they are related. A name is a channel, a channel is a permission, and "the disk, read-only" is simply a different service.

### The registry must be replaceable

Everything here is a service that can crash and be restarted (entries #9, #18, #26). The registry cannot be the exception — it is now on the path of every start.

It keeps **nothing that cannot be told again**. init holds the original of every handle and both ends of every channel, including the clients' channels to the registry. When the registry ends, init starts another and sends the whole list again:

```
[registry] asked to crash (a test)
[process] 34 "registry" exited with code 1
[init] registry ended with code 1 - restarting it
[registry] ready
[registry] shell may ask for: keys display init system files files.look disk.look
[registry] fs may ask for: disk
```

The shell's channel to the registry never closed — init holds the other end — so a lookup sent while there was no registry simply waits in the queue and is answered by the new one. And services the shell already holds are unaffected: **the registry is needed to *find* a service, not to *use* it.** Once you have the handle the registry is out of the picture. Crash it and running programs do not notice.

The boot test does this, and then — further down the same log, by luck of the test's order — the file service is itself restarted by the watchdog, and the *new* fs asks the *new* registry for the disk:

```
[init] fs has not answered for 4 s - stopping it
[init] fs ended with code -1001 - restarting it
[fs] ready: TaajFS on the disk, 804 of 1024 blocks free; a cache of 256 blocks
```

### fs must not guess

One decision in fs deserves a note. If the lookup of `"disk"` fails, what should fs do? It already has a fallback: with no disk it serves files from memory.

```rust
Some(Err(ServiceError::NotFound)) | None => None,          // no disk: files in memory
Some(Err(error)) => {                                      // could not ask
    println!("[fs] cannot ask for the disk: {}", error.describe());
    return 1;
}
```

"There is no disk" and "I could not find out" are different answers. If fs treated a registry that did not answer as "no disk", it would come up serving an empty file system in memory — and every file on the disk would *appear to be gone*, with nothing on the screen but a quiet log line. So when it cannot ask, it exits, and init starts it again. Failing loudly is the cheap option here because restart is cheap.

## The shell's two new commands

- `services` — the names that exist for the shell.
- `services NAME` — asks for one, as a program would, says whether it was there, and gives the handle straight back.
- `registry crash` — a test, like `fs stall`: the registry ends itself.

## The design, in one table

| Decision | Chosen | Rejected, and why |
|---|---|---|
| Who may look up what | A namespace per client channel | One global table — a name becomes ambient authority |
| A name outside the namespace | "Not found", same as a name that does not exist | "Not allowed" — says the thing exists, and for whom it does not |
| What moved to names | What a service uses of *other services* | Everything — an interrupt line is not a service; nothing else should find it |
| Where the logic is | A crate generic over the handle type, host-tested | In the program — the policy testable only by booting |
| Registry state | Nothing init cannot repeat | A registry that remembers — and loses it when it crashes |
| Who registers | init only, on an admin channel | Services registering themselves — needs a rule for who may claim a name |
| A handle the table does not keep | Returned to the caller, by type | Dropped — a channel that never closes |
| fs cannot reach the registry | Exit and be restarted | Fall back to memory — every file seems to vanish |
| Is the registry a separate process | Yes | Inside init — init stays the one process that only supervises |

## The bugs

Again none at run time: the first boot with the registry passed, crash and restart included. I am a little suspicious of three clean entries in a row, so, plainly: this one was *designed* more than the others. I did not write the registry program until the table had fifteen host tests, and the two things most likely to have bitten — the drain-init-first ordering, and fs guessing "no disk" — were found by reading the design for races before any code ran.

What the compiler caught:

1. **A closure borrowing what I then moved.** `let service = |name| services.lookup(name).ok();` borrows `services`; the struct I was building wanted to *own* `services` as a field. The fix is one of order — the field `services,` goes last in the struct literal, after the last call of the closure, and the borrow has ended by then. Rust evaluates struct fields in the order they are written.

## How I checked it

**On the host** (`make kernel64-test`, which now runs `registry`'s tests): 15 new tests.

- `protocol` (6): valid and invalid names; each message encoded and decoded back; every malformed message refused — cut short, a bad name inside, a client's request on the admin channel and the reverse.
- `table` (9): a client finds what is in its namespace; a name outside it is not there even though registered; a name inside it but unregistered is not there *until* it is registered; the list is exactly what a lookup would find; replacing a service or a namespace returns the old handle; removing a namespace returns its channel; whatever is refused is handed back; a full table refuses more but still replaces.

**In the boot test** (BIOS and UEFI):

- Everything that passed before passes — and all of it now depends on the registry, since the shell cannot print, read a key or run a program without having looked each service up. That is the main test.
- 7 new lines typed: `services` (the seven names, in order); `services files` (there); `services disk` (not there, and the registry's log says "not in its namespace"); `services nosuch`; `registry crash` (init restarts it); then `services files.look` and `cat welcome` — a lookup and a program after the restart.
- The check script confirms both namespaces were announced a *second* time: the new registry was told everything.
- The existing watchdog test restarts fs after the registry was replaced, and fs comes up on the disk.

`make test64` passed.

**Not tested:**

- I have not typed any of this into a `make run64` window myself.
- **A lookup made *while* the registry is down.** The design says it waits in the queue and is answered by the next registry. In the test the restart is faster than the typing, so the lookup after `registry crash` found the new registry already there.
- **The registry hanging rather than crashing.** It has no watchdog (see below).
- **A registry that keeps crashing**: init gives up on it after five restarts, like any service, and then nothing new can be found. Not exercised.
- **Full tables** — in the program, as opposed to the host tests.

## What is still missing

- **Programs still get their services the old way.** `ls` is handed the file service as a flagged startup handle by the shell (entry #21), chosen by the shell's lists of program names. This entry moved the *services'* wiring to names; the programs' is next. The plan: the shell asks the registry for a *narrower* namespace for each program it starts — `ls` gets one where `"files"` exists and means the read-only channel — and the program just asks for `"files"`. That is what will let "grant by program name" go.
- **The registry has no watchdog.** A crash is noticed (its process ends); a hang is not. It needs a ping, like the other services that answer requests.
- **Only init can register.** A service that init did not start — a network stack started later, say — has no way to announce itself.
- **A lookup's copy can be passed on.** The registry must give a client's copy the right to be *sent* (or it could not send it), so the shell's handle to the keyboard channel can now be handed to another process, which it could not before. The shell would lose it by doing so, and gains nothing — but it is a right init did not use to give. The kernel would need "send this handle, and strip this right as it lands" to close that.
- **No way to be told when a service appears**: a client that asks too early gets "not found" and must ask again.
- `given` still describes what it was given in the old terms.

## What I learned

- **A registry is the easiest place to lose a capability system.** The table of names is trivial; *whose* table it is, is the design.
- **Use the mechanism you already trust.** "Which channel did the request arrive on" answered the access question for the file service in entry #21 and answers it again here. No identities, no permission bits.
- **"Not found" is more honest than "not allowed"** when the design really is that the thing does not exist for you.
- **Split what a service owns from what it uses.** Only the second belongs in a registry.
- **State that can be repeated need not be saved.** The registry survives its own crash because it never knew anything init does not.
- **"I don't know" must not be silently read as "no".** fs and the missing disk.
- **Finding a service and using it are different dependencies.** Keeping them apart is what makes the registry's crash a non-event for running programs.
- **Let return types carry the handles.** If a function can decline or displace a resource, it should hand it back, so that forgetting it is a compile error and not a leak.
- In Rust: a generic parameter purely so tests can substitute a simple type; why `derive(Default)` over-constrains generics; `core::mem::replace`; `iter::once(..).chain(..)`; collecting `Option`s to fail a whole parse on one bad item; struct fields are evaluated in written order, which decides when a borrow ends.

---

## Testing note

```
make run64
```

Everything you have typed in earlier entries is now a test of this one: the prompt appearing at all means the shell found the keyboard and the screen by name.

**In the terminal** `make run64` was started from, near the top of the log:

```
[init] started: registry display fs blk shell kbd; supervising
[registry] ready
[registry] shell may ask for: keys display init system files files.look disk.look
[registry] fs may ask for: disk
```

**What exists for the shell**

| Type | You should see |
|---|---|
| `services` | `keys display init system files files.look disk.look` |
| `services files` | `files: there for the shell` |
| `services disk.look` | `disk.look: there for the shell` *(not in the automated test)* |
| `services disk` | `disk: no such service - for the shell` — and in the terminal, `[registry] shell asked for "disk": it is not in its namespace` |
| `services nosuch` | `nosuch: no such service - for the shell` |
| `services Files` | `Files: no such service - for the shell` *(not in the automated test)* |
| `services a b` | `usage: services [NAME]` *(not in the automated test)* |
| `ps` | A line for `registry`, running *(the automated test does not look for it by name)* |

**The registry is replaced, and nothing notices**

| Type | You should see |
|---|---|
| `registry crash` | `registry: asked to crash`; in the terminal `[registry] asked to crash (a test)`, `[init] registry ended with code 1 - restarting it`, then `[registry] ready` and both `may ask for` lines again |
| `echo still here` | `still here` — the shell's screen and keyboard were never the registry's |
| `cat welcome` | `Hello from the host` — a program is started and reads a file |
| `services files.look` | `files.look: there for the shell` — the new registry knows it |
| `services` | The same seven names as before |
| `registry` | `usage: registry crash (a test: init starts another)` *(not in the automated test)* |

**A service finds another through it**

| Type | You should see |
|---|---|
| `fs stall 6` then `ls` | A pause; in the terminal `[init] fs has not answered for 4 s - stopping it`, `[init] fs ended with code -1001 - restarting it`, and `[fs] ready: TaajFS on the disk, …` — the new fs asked the registry for the disk; then the listing |

**Too many crashes** *(not tested at all: please try it, and expect to restart QEMU after)*

| Type | You should see |
|---|---|
| `registry crash`, six times within 30 seconds | After the sixth: `[init] registry ended (code 1) 5 times in 30 s - giving up on it` |
| `echo still here` | `still here` — what the shell holds, it keeps |
| `services` | After ten seconds: `services: the registry is not answering` |

**`help`** — still one screen, now with `services [NAME]`.

Automated: `make test64`.
