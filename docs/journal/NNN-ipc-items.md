# The IPC items: measuring and speeding up messages, and the cycle leak

*Journal entry #NNN · a work-list change, not a feature. Written on TheJat-VM, on the branch `ipc-items`. The user asked how TheTaaJ's IPC is, and then for what was found to become work items. Seven `K?-ipc-...` items in `docs/who-does-what.md`, under "IPC: speed and the cycle leak", numbered at merge (rule 5).*

## Why

A look at the IPC code (`kernel64/src/ipc.rs`, `channel.rs`, `cap.rs`, and the system calls in `abi`) to answer the question "how is our IPC?" found the design right and its cost unexamined:

- **Right**: capabilities - a program reaches only the objects it was handed, with only the rights it was handed; inboxes bounded at 32 messages, so a sender can't grow kernel memory; handles moved all-or-nothing with a message; memory objects for bulk data without copying; `PeerClosed` when the other end goes, which is what lets init restart a crashed service; the rules tested on the host, and a documented lock order.
- **Not looked at**: speed. A request and its reply are four system calls (send and receive on each side) and two context switches; each message is copied in from the sender into a kernel `Vec` and out to the receiver; messages are at most 1 KiB, so streams are cut into 1023-byte pieces. Every file and network operation is such a request, so this is what rustc and a desktop will pay for.
- **One leak**: `ipc.rs` says it itself - channels can keep each other alive in a cycle (A's end in B's inbox, B's in A's) after their owners close every handle. Only the one-step cycle, an end sent through its own channel, is refused.

## The items

| Item | What |
|---|---|
| K?-ipc-bench | The benchmark: round trip, bulk through messages and through memory objects, both CPUs, KVM and emulated. First, so the rest is measured, not guessed |
| K?-ipc-call | One system call for a request and its reply, switching straight to the server's thread |
| K?-ipc-copy | One copy, no allocation per message |
| K?-ipc-big | Bigger messages or shared rings for streams |
| K?-ipc-send-wait | Waiting until a channel can be sent to |
| K?-ipc-priority | Priority inheritance across a call (after K20) |
| K?-ipc-cycles | The cycle leak |

They sit in MaK-WSL's part of the list, beside "Speed and timing": they are the kernel's core, and KVM measures them truly. They are not put in any machine's decided order: that is for the user to say.

## Design decisions

- **Measure first.** The benchmark comes before any change, and every change says what it moved. A faster IPC that nobody measured is a guess.
- **The design stays.** None of the items changes the capability model; they change what one message costs.

## How I checked it

- Each claim against the code: the four system calls (`CHANNEL_SEND`, `CHANNEL_RECEIVE`, no call that does both), the copies (`copy_from_user` into the message's `Vec`, `copy_to_user` out of it, in `ipc.rs`), the limits (`MESSAGE_MAX` 1024, `MESSAGE_HANDLES_MAX` 8, `CHANNEL_QUEUE_MAX` 32 in `abi`), the cycle note at the top of `ipc.rs`.
- Nothing was built or run: this changes only documents.

## What is still missing

- The items themselves, and a decision on which machine takes them and when.

## What I learned

- **A capability IPC's hard part is the rules, not the speed.** Lifetimes, rights and bounded queues are what are difficult to get right, and they are right here; the speed can be added under them without changing what a program may do.

## Testing note

Nothing to boot: this changes only `docs/`. To see the items:

```
grep -n 'K?-ipc' docs/who-does-what.md
```

| Look for | You should see |
|---|---|
| `K?-ipc-bench` ... `K?-ipc-cycles` | seven rows under "IPC: speed and the cycle leak", after K24 |
