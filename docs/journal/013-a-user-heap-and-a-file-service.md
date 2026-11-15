# A Heap for User Programs, and a File Service in Memory

*Journal entry #13 · `Vec` in ring 3, and the first server with a protocol*

Two things were missing before TheTaaJ could have files. Programs could not allocate memory — no `Vec`, no `String`, nothing whose size is decided at run time. And there was no service that owns named data and answers requests about it. This entry adds both. The files live in RAM for now; the disk is entries #14 and #15. What matters here is the *shape* of a service: its protocol, its limits, and what happens when it dies.

---

## Part 1: a heap in user space

### The same allocator, a different source of memory

The kernel already has a tested allocator (entry #4). An allocator is pure bookkeeping: *given regions of memory, hand out and take back pieces*. Only one thing differs between kernel and user space: **where the regions come from**.

So the allocator moved, unchanged, out of the kernel into its own crate, `heap/` — still tested on the host with the same 20,000-step stress test. Two users link it:

- the **kernel** feeds it frames it maps itself;
- a **user program** asks for memory the way it asks for anything — a system call. It creates a memory object (entry #11), maps it, and gives the mapping to the heap as a new region.

One implementation, tested once, used on both sides of the privilege boundary.

### Growing

When the heap runs out, the runtime adds a region. Each region is a separate mapping, and the kernel puts a guard page after every mapping, so two regions never touch and can never merge into one bigger block. To keep their number small, **each new region is at least as big as the whole heap already is** — doubling it — between 64 KiB and the 16 MiB limit of one memory object.

The memory object's handle is closed straight after mapping. This is the lifetime rule from entry #11 put to use: the *mapping* keeps the pages alive, and the handle would only occupy a slot.

Memory is not returned to the kernel while the program runs — true of most allocators — but **all of it is when the program exits**, with its address space.

With `#[global_allocator]` set in the runtime, `Box`, `Vec`, `String` and `BTreeMap` simply work in any program.

`heapcheck` proves it at every boot: growth across several regions, page-aligned allocations, freeing and reuse, and a **clean refusal** of a request beyond the limit (an error, not a crash). The self-test then checks every frame came back when it exited.

## Part 2: the file service

### Logic in a crate, a loop around it

The file service is split the way the kernel is:

- a **library crate** (then `ramfs`, today `fileserver`) holds everything that can be reasoned about: the table of files and the *protocol* — how a request and its reply are laid out in bytes. Pure logic, tested on the host.
- the **service** `fs` is a small loop: receive a message, hand it to the library, send back what it returns.

Clients use the same crate to build requests and read replies, so client and server **cannot disagree about the protocol**: there is one definition of it.

### The protocol

```
A request:   [op: u8] [name length: u8] [name] [number: u64 LE] [data]
A reply:     [status: u8] [payload]
```

`number` is an offset, a length or an index, depending on the operation: read, write, append, remove, list, size. One request per message, one reply per request. A file larger than a message is read or written in several requests, each with an offset.

### Limits everywhere

The service holds other programs' data in its own memory, so everything is bounded: the size of one file, the number of files, the total bytes. At boot it says so:

```
[fs] ready: files in memory, up to 256 of 1024 KiB each, 8 MiB in all
```

A request beyond a limit is refused with a status that says which. A service without limits is a service any client can kill by filling it.

### A reply channel per request

How does the reply find its way back? The obvious design — one shared reply channel — has two flaws. Replies for different clients could cross. And worse: if the service **dies** before answering, the client waits forever.

So **every request carries, as its one handle, the sending end of a fresh channel** for its reply. The client waits on the other end.

- Replies cannot cross: each has its own channel.
- If `fs` dies before answering, the reply end *dies with it* — it was in `fs`'s handle table, or still in the queue — and the client hears **`PeerClosed`**. The rule from entry #9 does the work: no timeouts, no special cases.

It costs a channel per request. They are cheap, and the boot tests check that none is ever left behind.

### Careless and hostile clients

The service must survive any message at all. The test client sends requests with **no reply channel**, with **extra handles**, with **garbage bytes**, with a name length pointing past the end of the message. Each is refused or dropped; the extra handles are closed, not leaked; the service keeps running.

### Restart

init holds both ends of the request channel, as it does for the keyboard (entry #10). A restarted `fs` takes over the same queue, and clients keep their handles. But the *files* are gone — they lived in the dead process's memory. That is the honest limit of a RAM disk, and the reason for the next two entries.

## The shell gets files

`ls`, `cat`, `write`, `append`, `rm`, through `taaj::files` — the client side of the protocol, which any program can use. (At this step they were built into the shell; entry #16 makes them separate programs.)

## How I checked it

A new kernel self-test starts `fs` together with `fscheck`, a test client, **twice**:

- files spanning many messages;
- a read that ends *exactly* on a message boundary (the classic off-by-one);
- a listing of 104 files, which needs several replies;
- every refusal: too big, too many, no such file, bad name;
- the careless requests above.

Afterwards: **0 channels and no frames left**.

## What I learned

- **Reuse by moving the boundary.** The allocator did not need a second implementation; it needed its dependency — where memory comes from — turned into a parameter.
- **A protocol is an interface; give it one definition.**
- **Design for the peer dying.** A reply channel per request turns "the server crashed" into an ordinary error at the client, for free.

---

## Testing note

```
make run64
```

In the terminal log:

| Line | Meaning |
|---|---|
| `[process] N "heapcheck" exited with code 0` | The user heap grew, reused and refused correctly |
| `[fs] ready: files in memory, up to 256 of 1024 KiB each, 8 MiB in all` | The in-memory file service (started by the self-test) |
| `[fscheck] long files, boundaries, appends, 104-file listing, refusals, careless requests: all passed` | The protocol, from a client's side |
| `[selftest] files: fs and fscheck, twice - every check passed, 0 channels and no frames left` | …and nothing leaked |

In the QEMU window (today these commands are programs on the disk, but the protocol underneath is this entry's):

| Type | You should see |
|---|---|
| `write a one` | `a: 4 bytes` |
| `append a two` | `a: 8 bytes` |
| `cat a` | `one` then `two` |
| `ls` | `a` in the list, 8 bytes |
| `rm a` then `cat a` | `cat: a: no such file` |
| `write` + a name longer than 28 characters + ` x` | A refusal naming the problem, not a crash |

The in-memory store itself is what the self-test lines above exercise at every boot; with a disk attached, the service uses the disk instead (entry #15).

Automated: `make kernel64-test` (heap, protocol and stores), `make boot64-test`.
