# Read-Only Files, a Service Stalled on Purpose, and a Kernel Too Big to Boot

*Journal entry #22 · a second channel to the file service; `fs stall`; the bootloader's 2 MiB limit*

Entry #21 ended with two things undone. "The files" was all or nothing: `cat`, which only reads, held the same power as `rm`. And the timeout that keeps the shell alive when the file service hangs had never met a hung file service — only a stand-in.

This entry does both. And then, while testing them, the system stopped booting — for a reason that had nothing to do with either, and had been approaching quietly for months. That is the third part, and the most useful lesson of the three.

---

## Part 1: files, to read only

### Where can "read-only" live?

A handle to the file service is the send end of a channel. Its rights say *send* — the kernel knows nothing about files, so it cannot know that one message means "read" and another "remove". The restriction has to be enforced by the service. The question is how the service knows which client is allowed what.

Three ways:

1. **The client says so in the request** — "I am read-only". Useless: a client that wants to cheat leaves it out.
2. **The service keeps a table of clients.** But channels are anonymous. A request does not say who sent it — on purpose; identity is not how this system grants things.
3. **A different channel for each kind of access.** The service listens on two. Requests arriving on the second may only read. *Which channel a client holds is its permission.*

The third needs no identity, no table and no new kernel feature. It is the same idea the disk driver already uses (entry #15: `blk` serves a read-write channel for `fs` and a read-only one for the shell). And it composes with everything else: a read-only handle can be duplicated and passed on, and stays read-only, because it *is* a different object.

### In the service

`fs` now gets a second request channel from init and waits on both with `object_wait` (entry #12):

```rust
let from = match taaj::object_wait(&channels[..watched]) { ... };
let access = if from == 0 { Access::ReadWrite } else { Access::ReadOnly };
```

The rule itself is in the protocol library, in one place, before any operation runs:

```rust
// Checked here, once, before anything is done - not in each
// operation, where a new one could forget.
let changes = matches!(request.op, protocol::WRITE | protocol::TRUNCATE | protocol::REMOVE | protocol::STALL);
if changes && access == Access::ReadOnly {
    return Err(Status::ReadOnly);
}
```

Two details worth their lines:

- **One check, at the door.** If each operation checked for itself, the next operation I add could forget to. A single list of "what changes things" is one place to get right.
- **Refused before the name is looked at.** A read-only client that tries to remove `secret` gets `ReadOnly` whether or not `secret` exists. Answering `NotFound` for one and `ReadOnly` for the other would tell it which files exist through a channel that — one day — might not be allowed to list them.

A new request, `ACCESS`, lets a client ask what its channel allows.

### In the shell

The shell now holds both channels, and its lists split in two:

```rust
/// These read; they get the channel that cannot change anything.
const LOOK_PROGRAMS: &[&str] = &["ls", "cat"];
/// And these change files.
const CHANGE_PROGRAMS: &[&str] = &["write", "append", "rm"];
```

`with files COMMAND` still grants everything; the new `with readonly COMMAND` grants the reading channel. And the shell took its own medicine: it only ever *reads* files — the programs it runs — so its own lookups now go through the read-only channel too.

To a program nothing changed. `taaj::file_service()` returns the files; whether they can be changed is a property of the handle it was given, discovered when it tries:

```
taaj> with readonly write saved lost
write: saved: the files are read-only here
```

### What it does not do yet

It is still *all* the files. One file, or one directory (there are no directories yet), needs the service to create a channel per grant, on request — "give me a handle that can read only `notes`". The structure is now right for that: a channel is already a permission; what is missing is making them on demand.

And the service always looks at the read-write channel first. A flood of writes could starve readers. With one user at one keyboard it cannot happen yet; it goes on the list.

## Part 2: stalling a service on purpose

### Why a test hook

To test what clients do when a service is stuck, a service has to get stuck — on demand, for a known time. F12 does this for a *crash* of the keyboard driver (entry #10). A hang needed its own.

`STALL` is a request: the service answers it — so the one who asked is not the one left waiting — and then sleeps for the given number of ticks.

Is that a hole? A client can now make the file service stop. Three limits keep it honest:

- at most **30 seconds** per request;
- only on the **read-write** channel — and a client with that can already keep the service busy as long, by writing large files;
- it is in the protocol, visible, and logged by the service every time: `[fs] asked to stall: answering nothing for 600 ticks`.

A test hook that grants no authority its holder lacked is one I am comfortable shipping.

### The real scenario, at last

```
taaj> fs stall 6
fs: the file service will answer nothing for 6 s
taaj> ls
run: ls cannot be looked up: the file service is not answering
taaj> ls
   ... the listing ...
```

The first `ls` waits its five seconds and gives up — the shell is alive, says what is wrong, and takes the next command. The second `ls` waits out the last second of the stall and is served.

One thing is worth noticing in the log. The first `ls` left a request in the service's queue when it gave up. When the service woke, it answered that request — to a reply channel whose other end was closed. The send failed, harmlessly, exactly as entry #21 argued it would. The argument is now an observation.

**What this still does not give:** recovery. If the service were stuck *forever*, every command would take five seconds to fail, forever. Nobody notices and restarts it — init restarts what *ends*, not what hangs. A watchdog in init (ask each service "are you there?", restart the ones that do not answer) is the missing half, and now there is a way to test one.

## Part 3: the kernel that was too big to boot

### What happened

With parts 1 and 2 working — BIOS boot test passing, UEFI passing — I ran the full suite. Five of the six **fault tests** failed. Not with a wrong report: with *nothing*. The serial log of each was zero bytes long. The kernel had not printed its first line.

Nothing I had changed was anywhere near the fault tests, or near early boot.

### Finding it

What I knew: the normal kernel booted. The `fault-double` kernel booted. Five others did not. Same source, different Cargo features.

So I compared the files:

```
973,608 bytes   normal kernel        boots
973,048 bytes   fault-double         boots
978,488 bytes   fault-nx             nothing
```

A size cliff — somewhere in five kilobytes. Something that fails by *size* and fails *before the kernel's first instruction* is not in the kernel. It is in whatever loads the kernel.

The kernel's own boot log had been printing the relevant number at every boot since entry #2:

```
kernel         0xff0f0 bytes at physical 0x100000
```

Loaded at 1 MiB, `0xff0f0` bytes long: it ended at `0x1ff0f0`. That is **3,856 bytes below 2 MiB**. And in the bootloader:

```c
#define RAMDISK_LOAD_ADDRESS 0x200000
...
#define LOAD_MAX_ADDRESS   RAMDISK_LOAD_ADDRESS
...
if (End < Dest || Dest < LOAD_MIN_ADDRESS || End > LOAD_MAX_ADDRESS) {
    boot_print("[load_elf64], Segment outside 1 MiB .. ramdisk, refusing.\n");
    return 0;
}
```

The loader puts the ramdisk at 2 MiB, so it refuses a kernel that reaches 2 MiB. That is a *correct* check — I wrote it so a kernel could never be silently overwritten. It even prints why. **On the screen.** The tests run headless and read the serial port, where the loader prints nothing.

So: a correct refusal, with a clear message, shown where no test looks.

### Why the kernel was that big

The kernel image contains every built-in program — init, the services, the test programs — as complete ELF files, 17 of them. Each feature for months added a few kilobytes: a new check in `fscheck`, a new command in the shell. This entry's changes pushed the fault builds (which carry a little extra test code) over the line. The normal build was next: it had under 4 KB left. The *following* feature would have made TheTaaJ unbootable, and it would have looked like that feature's fault.

### Three fixes, for three different problems

**1. The silence.** A limit that fails silently will be hit again. It is now checked where it can speak — at link time:

```
KERNEL_LOAD_LIMIT = 0x200000;
ASSERT(__kernel_end - KERNEL_WINDOW <= KERNEL_LOAD_LIMIT,
    "the kernel image ends above 2 MiB, where the bootloader refuses to load it - see linker.ld")
```

I checked that it fires by lowering the limit: the build stops with exactly that sentence. A boot that never starts became a build error with a reason.

**2. The headroom.** The user programs were built for speed with their symbol tables included. They are now built for size (`opt-level = "s"`, link-time optimisation, stripped). The programs shrank by about a quarter in total, and the kernel image from `0xff0f0` to `0xd00f0` bytes: roughly **190 KB of room** where there were 4.

The cost: a fault address in a user program no longer maps to a function name without rebuilding that program unstripped. The Cargo file says how.

**3. The cause — not fixed here.** Both of those buy time. The real problems are two, and both are now written down:

- the loader's layout is fixed: kernel from 1 MiB, ramdisk at 2 MiB, staging at 3 MiB. A kernel should be able to be any size that fits in memory. That is a change to both loaders.
- the kernel carries programs it does not need to. Only init and the services required to reach the disk must be in the image; the test programs and the shell could be files. That is the natural direction anyway (entry #15 began it).

### Was this the "rare BIOS hang"?

Entry #11 recorded a boot that, very rarely, printed nothing at all. It is tempting to call it solved. It is not: that one was *intermittent* with the *same* kernel; this one is deterministic by size. Different bug. It stays on the list.

## How I checked it

- **Host tests** (`fileserver`): a read-only channel reads, sizes and lists; every changing request — write, append, truncate, remove, *creating* by write or truncate, stall — is refused and changes nothing; refusal comes before the name is examined; `ACCESS` answers correctly on both; a stall beyond the limit is refused.
- **`fscheck`**, twice per boot, from ring 3 over real channels: the same rules; then a stall of 20 ticks during which a client with 5 ticks of patience must time out and a patient one must be served.
- **Boot tests** (BIOS and UEFI): `with readonly given` reports `to read only`; `with readonly write` and `with readonly rm` are refused, and the file is intact afterwards — checked in the OS with `cat` and again on the host from the disk image; `fs stall 6` then `ls` must report the service not answering, then `ls` must work.
- **The size check**: forced to fail once, by hand.
- `make test64`: all passed, fault boots included.

## What I learned

- **A permission can be a place.** Two channels instead of one flag: nothing to forge, nothing to look up.
- **Check at the door.** One list of what changes things, consulted before anything runs.
- **A message nobody reads is not a message.** The loader's refusal was correct and clearly worded, and still left five tests failing with no explanation, because it went to the screen and the tests read the serial port. An error has to be delivered where failure is looked for.
- **Limits get approached gradually and hit suddenly.** The number that would have warned me was printed at every boot. Nothing compared it with anything. Now the linker does.
- **When something fails that you did not touch, compare what differs** — here, sizes — before reading code.

---

## Testing note

```
make run64
```

**Read-only files**

| Type | You should see |
|---|---|
| `given` | `given: the screen, no files` |
| `with files given` | `… and the files (N of them), to read and to change` |
| `with readonly given` | `… and the files (N of them), to read only` |
| `write t hello` | `t: 6 bytes` |
| `with readonly cat t` | `hello` |
| `with readonly write t gone` | `write: t: the files are read-only here`, `write exited with code 1` |
| `with readonly rm t` | `rm: t: the files are read-only here` |
| `cat t` | `hello` — untouched |
| `with` | `usage: with files COMMAND, with readonly COMMAND` |
| `rm t` | Nothing (it worked: `rm` is given the changing channel) |

**A stalled file service** — this one you can watch:

*(Since entry #23, init notices the silent service and replaces it, so the first `ls` below now pauses and then succeeds. The table shows what this entry's code did; use entry #23's testing note for what you see today.)*

| Type | You should see |
|---|---|
| `fs stall 10` | `fs: the file service will answer nothing for 10 s` |
| `ls` (at once) | Nothing for about 5 seconds, then `run: ls cannot be looked up: the file service is not answering` — and a prompt |
| `echo still here` | `still here`, immediately: the shell is alive |
| `uptime` | Works: it does not need the file service |
| `ls` again, after the 10 s are up | The listing |
| `fs stall 99` | `usage: fs stall SECONDS (1 to 30)` |

In the terminal log: `[fs] asked to stall: answering nothing for 1000 ticks`, later `[fs] answering again`.

**The kernel's size:** in the terminal log near the top, `kernel         0xd00f0 bytes at physical 0x100000` (the exact figure will move). It must stay below `0x100000`; if it ever would not, `make` now fails with *"the kernel image ends above 2 MiB…"* instead of producing a system that does not boot.

Automated: `make test64`.
