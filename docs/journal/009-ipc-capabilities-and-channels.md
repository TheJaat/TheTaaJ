# IPC: Capabilities, Handle Tables and Channels

*Journal entry #9 · how isolated processes are allowed to cooperate*

After entry #8, processes were perfectly isolated — and so perfectly useless to one another. In a microkernel, drivers, the file system and the screen are all separate processes; the system *is* the conversation between them. This entry builds that conversation, and with it the security model of everything that follows.

Two ideas: **channels** carry messages; **capabilities** decide who may use what.

---

## The question behind every OS security model

*How does the kernel decide whether a process may do something?*

The familiar answer (Unix, Windows) is: objects have **global names** — paths, process IDs — and an access list is checked when a name is used. Any process may *name* any object; the check comes afterwards. The trouble is "ambient authority": a program runs with all of its user's rights all the time, and a fooled program misuses them. (Entry #16 meets this as the *confused deputy*.)

TheTaaJ uses the other answer:

> **Holding a handle *is* the permission.** There is no global namespace to look things up in and no list to consult. If a process has a handle to a channel with the SEND right, it may send. If it has no handle, that channel does not exist as far as it is concerned.

Authority flows only along handles. Who can do what is visible in who holds what.

## Handle tables

Each process has its own table. A handle is just a number naming a slot in *that process's* table. Three properties make it safe:

**1. Unforgeable.** The number means something only in the holder's table, and is checked on every use. Guessing gets `InvalidHandle` — or at worst one of the process's own handles, never someone else's.

**2. No reuse confusion.** Each slot has a **generation**, increased whenever the slot is freed, and the handle value includes it:

```rust
/// A handle value, as a process sees it: generation << 32 | (slot + 1).
fn encode(slot: u32, generation: u32) -> Handle {
    (u64::from(generation) << 32) | (u64::from(slot) + 1)
}
```

Without it: a program closes handle 3, opens something new which lands in slot 3, and a stale copy of "3" elsewhere in the program now silently operates on the *wrong object*. With generations, the stale handle is simply invalid. A use-after-close becomes an error instead of a mystery. (The `+ 1` keeps 0 from ever being a valid handle.)

**3. Rights only shrink.** Each handle carries rights — `SEND`, `RECEIVE`, `DUPLICATE`, `TRANSFER` at this step. Duplicating a handle may drop rights, never add them. So a process can hand out a weaker version of what it holds: "you may send to this, but not receive, and not pass it on".

The table is generic over what it stores, so all these rules are tested on the host with plain values.

## Channels

A channel is a two-ended pipe for **messages**. Each end has an inbox. Sending on one end puts a message in the other end's inbox.

- **Messages, not bytes.** A message arrives whole or not at all; the receiver never has to find boundaries in a stream.
- **Bounded inboxes.** A queue has a fixed capacity; sending to a full one returns `QueueFull` at once. This is **backpressure**. The alternative — unbounded queues — lets one runaway sender consume all kernel memory. Memory use per channel has a hard ceiling.
- **Send never blocks; receive may.** A sender can therefore never be held hostage by a receiver that stopped reading.
- **`PeerClosed`.** When one end's last handle closes, the other end is told: receivers get the remaining messages, then `PeerClosed`. And since **a process's handles all close when it exits or is killed**, a client waiting on a service that crashed is woken with an error instead of waiting forever. This one rule is what makes crash recovery (entry #10) possible.

## Sending handles in messages

A message carries bytes **and handles**. A sent handle *moves*: it leaves the sender's table and appears in the receiver's. This is the only way authority travels — and it needs the `TRANSFER` right, so the holder of a capability decides whether it can spread.

Moving several is **all-or-nothing**: if any handle is invalid or not transferable, or the queue is full, none moves and the sender still has them all. A half-moved set would be a leak or a theft.

This is how everything is wired from here on: to give a program the screen, give it a handle to a channel whose other end the display service holds.

## The bug: a chain 2000 deep

Since a channel's queue can hold messages containing handles to *other* channels, objects can nest: a channel, holding a message, holding a channel, holding a message…

Rust frees such a structure by recursion: dropping the outer value drops the inner, which drops the next. I wrote a test that built a chain **2000 deep** and dropped it. The kernel stack — 64 KiB — overflowed. The guard page from entry #7 caught it, which is how I learned what had happened rather than staring at a hang.

This is not a curiosity: any program could build such a chain on purpose and crash the kernel by exiting. The fix is to destroy **iteratively**: when a channel dies, the handles in its queued messages are moved onto a work list, and a loop processes the list. Depth no longer costs stack. The 2000-deep chain is now a permanent test.

## New system calls

`channel_create`, `channel_send`, `channel_receive`, `handle_close`, `handle_duplicate`. The kernel also learned to write into user memory safely (`copy_to_user`, with the same translate-and-check as reads in entry #8), and a new process receives its first handle — a *bootstrap* handle — at startup.

## How I checked it

Two programs:

- **`echod`**, a service: receives a message, sends it back.
- **`echoc`**, its client — which is really a test suite running in ring 3. It checks every rule from the outside: a send-only handle cannot receive; a duplicate cannot gain rights; a non-transferable handle cannot be sent; a closed handle stays dead after its slot is reused; filling a queue gives `QueueFull`; a handle sent across arrives and works. Then it tells `echod` to **crash**, and checks that it gets `PeerClosed` rather than hanging.

The kernel self-test runs the pair twice and requires that not one frame and not one channel is left afterwards.

## What I learned

- **Capabilities turn "who may do what" into a data-flow question.** I can read a service's authority off the list of handles it was given. Every later entry leans on this.
- **Bound everything a stranger can fill.** Queues, tables, nesting depth.
- **Recursion depth is attacker-controlled** whenever the data structure is.

---

## Testing note

```
make run64
```

In the terminal log:

| Line | Meaning |
|---|---|
| `[process] N "echod" killed: page fault` | The service crashed on purpose… |
| `[process] N "echoc" exited with code 0` | …and its client noticed, and passed every check |
| `[selftest] ipc: echo service and client - rights, transfer, backpressure, crash isolation; 0 channels left` | Twice over, nothing leaked |

In the QEMU window:

| Type | You should see |
|---|---|
| `ps` | A line ending `N channels`. Note the number |
| `ls` and `primes`, a few times each, then `ps` again | **The same number.** Every command creates channels (a reply channel per request, an arguments channel per program); all are gone when it ends |

Everything you type travels over channels: keyboard driver → shell → display service. The system working at all is the larger test.

Automated: `make kernel64-test` (handle tables, channels), `make boot64-test`.
