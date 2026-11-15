# Every Service Watched: Pings for the Display and the Keyboard Driver, and Four Hangs on Purpose

*Journal entry #24 · the watchdog reaches the services that answer nobody*

Entry #23 left an honest list of what the watchdog did not cover. Three items on it are this entry:

- the **display** and the **keyboard driver** were not watched at all — they answer no requests, so there was nothing to ask;
- the **disk driver's** replacement was "reasoning, not evidence" — nothing could hang it on purpose;
- and so the claim that the file service *outlasts* a replaced disk driver had never been seen to hold.

After this entry, four of the five services can be hung deliberately, and the boot tests hang all four and require the system to carry on. Getting there turned up two bugs that had nothing to do with watchdogs, which is the usual way.

---

## Services that answer nobody

The watchdog's rule (entry #23): *health is answering, checked through the door the clients use.* That works for `fs` and `blk`, whose whole job is request and reply.

The display is different: text goes in, pixels come out, and nothing ever comes back. The keyboard driver is the opposite: it only sends. Neither has a question that can be put to it.

So each got one: a **ping channel**. init keeps the sending end; the service gets the receiving end as one more startup handle. The convention is one constant in the ABI:

```rust
/// A ping: the one message a service's ping channel carries - this
/// byte, and one handle, a channel to answer on. The answer is one
/// byte.
pub const PING: u8 = 1;
```

and one function in the runtime that any service can call from its loop:

```rust
pub fn answer_pings(pings: Handle) -> bool
```

### Does a separate channel break the rule?

Entry #23 argued against a private health line: something that could say "alive" while the real work was stuck. A dedicated ping channel sounds like exactly that.

The distinction is *who answers*. A separate **thread** answering pings would lie — it could run happily beside a stuck main loop. Here there is no second thread. The pings are answered by the **same loop** that draws text or reads keys, at one particular point in each turn. If that loop is stuck, the answer does not come. The channel is separate; the liveness being measured is not.

### Why not the display's own queue?

I first meant to send pings down the display's text channel, as with `fs`. One case killed it: **a flood**. `cat` of a big file keeps the display's queue full for seconds. A ping could not get in; the watchdog would see silence from a display working flat out, and kill it. Slow is not dead — entry #23's warning, about to come true.

A channel of its own is never full. And the display's loop looks at it *between batches*:

```rust
// First, between batches, whoever asks if this loop is turning.
if let Some(channel) = pings {
    if !taaj::answer_pings(channel) {
        pings = None;
    }
    // Text or a ping, whichever comes; a ping is for the top.
    match taaj::object_wait(&[messages, channel]) {
        Ok(0) => {}
        Ok(_) => continue,
        Err(_) => return 5,
    }
}
```

The order matters. `object_wait` returns the *first* ready handle, and under a flood the text channel is always ready — so if pings were only answered when `object_wait` pointed at them, they would starve. Answering at the top of every turn means a busy display answers once per batch of 64 messages.

The same thought fixed a hole in the watchdog itself. For `fs` and `blk` a ping still goes into the request queue, and I had written "a ping that cannot be sent is as good as sent" — start the clock and stop trying. That would kill a service whose queue was full for one instant at the wrong moment. Now an unsent ping is **retried every tenth of a second** while the clock runs; only a queue that stays full for the whole patience counts as silence.

### The keyboard driver

It used to sleep in `interrupt_wait`. Now it waits for *either* the interrupt *or* a ping — `object_wait` already accepts interrupt objects (entry #12) — and only then takes the interrupt:

```rust
match taaj::object_wait(&[interrupt, channel]) {
    Ok(0) => {}                        // a key: fall through and take it
    Ok(_) => { /* answer pings */ continue; }
    Err(_) => return 5,
}
```

## Hanging things on purpose

| Service | How | Why that way |
|---|---|---|
| `fs` | `fs stall N` (entry #22) | a request in its protocol |
| `blk` | `disk stall N` | the same, in the block protocol |
| `display` | `display stall N` | a message type beside INFO and TEXT |
| `kbd` | **F11** | it has no input channel to ask through — but it has keys. F12 crashes it (entry #10); F11 now makes it loop forever |

All capped at 30 seconds, all logged by the service when they happen.

The two new protocol hooks deserve the same question I asked of `fs stall`: do they give anyone power they lacked? The display's does, slightly: every program holds the screen, so any program can now freeze it — until the watchdog replaces it about three seconds later, with a blank console. But any program can already clear the screen, or flood it. The disk's is reachable from the shell's read-only channel, which only the shell holds and cannot pass on. I am content with both, for a system at this stage; they are the kind of thing to compile out of a release build one day.

## What the tests now do

In one boot, in order:

```
fs stall 6        then ls               -> fs replaced; ls answered by the new one
disk stall 6      then write during x   -> blk replaced; the write completes
display stall 6   then echo unseen      -> display replaced
F11               then echo keys        -> kbd replaced; the line arrives
```

From the log of that boot:

```
[blk] asked to stall: answering nothing for 600 ticks
[init] blk has not answered for 2 s - stopping it
[init] blk ended with code -1001 - restarting it
during: 2 bytes
```

That last line is the one I wanted. `write during x` reached the file service, which asked the disk driver to write — a driver that was, at that moment, playing dead. The request sat in the driver's queue through the kill and the restart; the new driver found it and wrote the sector; the file service committed; the program printed `2 bytes`. Three processes deep, across a replaced driver, and the only visible effect is a pause.

**And the file service was not killed.** That was the untested claim: with 4 seconds of patience against the driver's 2, `fs` — silent while it waited on `blk` — should survive. The test script now checks it: `fs has not answered` may appear **at most once** in a boot (its own stall), and the watchdog may fire at most four times in all. A fifth would be a false alarm, and fails the test.

## Two bugs found on the way

### 1. One message could wedge the display forever

The display received messages with no room for handles. The kernel's rule (entry #9) is that a message which does not fit the receiver's buffers **stays queued** — so the receiver can try again with more room. The display never did. A single message carrying a handle would sit at the head of its queue, refused on every attempt, and no text would ever be drawn again.

And *every program holds the screen*. Any of them could have sent it, by accident or not.

`fs` and `blk` already guarded against this ("room for any message, so none can get stuck"). The display, written earlier, did not. It now takes whatever handles arrive and closes them.

I found this only because I was about to send the display a message with a handle on purpose, and went to see what it would do.

### 2. init died, and said nothing

The first boot with four watched services printed this and went dark:

```
[init] started: display fs blk shell kbd; supervising
[process] 33 "init" exited with code 3
```

init's wait set had grown to ten handles: five processes, its request channel, four answer channels. The kernel's limit for one `object_wait` was eight — it had borrowed the constant for "most startup handles", which happened to be big enough until now. The call failed, and init's response to a failed wait was `return 3`.

Two fixes:

- the limit is its own constant now (`WAIT_HANDLES_MAX`, 32), with a comment saying who needs it;
- init says why before it gives up: `[init] cannot watch the services: … - giving up`.

The deeper problem is still there, and it is entry #23's "who watches init?" made concrete. When init exits, its handles close; every channel it was keeping alive closes; the services see their peers vanish and exit, one after another, each *correctly*. The whole system switched itself off in good order over one bad argument. The kernel should treat init's exit as what it is — the end — and say so loudly. That is on the list.

## The ghost, seen again

During the final full test run, one fault-test boot produced an **empty serial log** — the same kernel image that booted correctly three times straight afterwards, and forty more times when I looped it trying to catch it.

This is the "rare BIOS boot that prints nothing" from entry #11, and it is *not* the size limit of entry #22 (that one is deterministic). What I can add to the record: this time QEMU *exited* rather than hung — the script takes a screenshot of a hang, and there was none — which points at a CPU reset very early, in or before the loader. The evidence was overwritten by my own reruns before I thought to keep it.

So the one change I could make: when a fault test fails, the scripts now **keep the logs** (`*.failed`) instead of letting the next run replace them. A bug that appears once in hundreds of boots will not be caught by looking for it. It has to be caught by being ready.

## Still not covered

- **The shell.** It is the one service not watched: it talks to a person, and "is it answering?" has no obvious meaning while it waits for a program the user started. A hung shell needs a reboot.
- **A restarted display starts blank.** The text that was on the screen is gone, and the prompt does not reappear until something prints. Keeping the screen's contents across a restart means keeping them somewhere outside the display process.
- **Keys pressed while the keyboard driver is hung are lost** — the controller buffers one byte.
- Poisoned requests, slow-but-alive services, and init itself: as in entry #23.

## How I checked it

- **Boot tests, BIOS and UEFI**: the four hangs above, each required to produce its `has not answered … stopping it` and `ended with code -1001 - restarting it`, and each followed by proof of service: `ls` output, `during: 2 bytes`, a line typed through the new keyboard driver, and at the very end the screenshot check on the new display (cleared, then one line of text, no stale pixels).
- **No false alarms**: at most one `fs` alarm and four alarms in total per boot, enforced by the script; three BIOS boots in a row gave exactly four each. The same session runs `cat` of 300 lines and `disk check` — the display's and driver's heaviest work.
- **The leak check**: `ps` before and after reports identical counts (14 channels now).
- `make test64`: the host tests and the BIOS and UEFI boots passed, and five of the six fault boots. The sixth was the silent boot described above; the same image passed on every one of 43 reruns. So the suite has **not** passed in a single clean run with this change - only in pieces.

## What I learned

- **Measure the loop, not the process** — and a separate channel is fine as long as the same loop serves it.
- **Starvation hides in "first ready wins".** Anything that must not starve gets checked on every turn, not only when the wait points at it.
- **Old code has old assumptions.** The display predated the "room for any message" rule; the wait limit borrowed a constant that was only coincidentally large enough. Both were found by pushing something new through them.
- **A supervisor's failure is the system's failure**, and it should be the loudest message there is, not `exited with code 3`.
- **Keep the evidence.** Rare failures are caught by preparation.

---

## Testing note

```
make run64
```

Do these one at a time, watching the terminal log beside the QEMU window.

**The disk driver**

| Type | You should see |
|---|---|
| `disk stall 20` | `disk: the driver will answer nothing for 20 s` |
| `write t hello` (at once) | A pause of 2–3 seconds, then `t: 6 bytes` |
| `cat t` | `hello` |
| `ps` | `blk` with a new process number; **`fs` with the same number as before** |

Log: `[blk] asked to stall…`, `[init] blk has not answered for 2 s - stopping it`, `[init] blk ended with code -1001 - restarting it`, `[blk] virtio disk at …`. There must be **no** `fs has not answered`.

**The display**

| Type | You should see |
|---|---|
| `display stall 20` | `display: the screen will show nothing for 20 s` — then the screen freezes |
| (wait about 3 seconds) | The screen **clears**: title band, empty console. That is the new display |
| Press Enter | A prompt appears |
| `echo back` | `back` |

Log: `[display] asked to stall…`, `[init] display has not answered for 2 s - stopping it`, `[display] painted the 1024x768 screen…` a second time.

**The keyboard driver**

| Do | You should see |
|---|---|
| Press **F11** | Nothing on screen. Log: `[kbd] F11: simulating a driver bug - looping forever` |
| Type immediately | Nothing — the driver is hung (these keys are lost) |
| Wait about 3 seconds, type `echo keys` | It appears and runs |

Log: `[init] kbd has not answered for 2 s - stopping it`, `[kbd] PS/2 keyboard ready (driver in ring 3, process N)` with a new number.

**The file service** — as in entry #23: `fs stall 20`, then `ls` pauses and lists.

**No false alarms**

| Do | You should see |
|---|---|
| The flood test from entry #19 (`time cat big`) if you still have `big` on the disk | It completes; **no** `display has not answered` in the log |
| Ordinary use for a few minutes | `has not answered` never appears without a stall or F11 |

If the watchdog ever fires on its own, that is the most useful thing you could tell me: which service, and what was running.

Automated: `make test64`.
