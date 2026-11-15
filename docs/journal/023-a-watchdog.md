# A Watchdog: Replacing a Service That Has Stopped Answering

*Journal entry #23 · init notices silence, not only death*

Since entry #10, init restarts a service that **ends**. That covers crashes: a bad pointer, a broken rule, an exit. It does not cover the other way software fails — by *not* ending. A service stuck in a loop, or waiting for something that will never arrive, is alive as far as the kernel can tell. Its process exists; its handles are open; nothing wakes init.

Entry #21 made clients survive that: after a time limit they give up and say so. Entry #22 gave me a way to cause it on purpose (`fs stall`). But surviving is not recovering. With the file service stuck for good, every command would fail after its timeout, forever, on a machine that needed only one process replaced.

This entry closes that gap. init now asks its services, regularly, whether they are there — and replaces the ones that do not answer.

---

## What "alive" has to mean

"The process exists" is the kernel's notion of alive, and it is the wrong one. A service is useful only if it **answers requests**. So the only honest health check is a request.

That immediately rules out anything special. If init had a private "are you alive?" line into each service — a separate thread, a flag in shared memory — it could report health while the request loop was stuck. The check must go **through the same door the clients use**, and be answered by the same loop that answers them.

So a ping is an ordinary request of the cheapest kind, on an ordinary channel:

- to `fs`: *"what does this channel allow?"* (`ACCESS`, from entry #22), on the read-only channel;
- to `blk`: *"what disk is this?"* (`INFO`).

Neither service knows it is being watched. No protocol changed. If the service can answer that, it can answer a client.

## The rule init must not break

Entry #15 set down a rule: **init must never wait on a service it supervises.** If init blocks on `fs` and `fs` hangs, init hangs, and then nothing can restart anything.

A watchdog *asks services questions*. Done naively — send, then wait for the reply — it would be the most direct violation of that rule possible.

It is safe only because of timed waits (entry #19). init's main loop was already one `object_wait` over "a service ended" and "the shell wants a program run". The answers to pings became **more entries in the same set**, and the wait got a deadline:

```rust
let index = match taaj::object_wait_until(&watched[..=count], wake) {
    Ok(Some(index)) => index,
    Ok(None) => continue, // time for the watchdog
    Err(_) => return 3,
};
```

init never waits *for an answer*. It waits for *whatever happens next* — a death, a request, an answer, or the clock — and then looks at the time. A service that never answers costs init nothing but a deadline passing.

## The state per service

```rust
struct Watch {
    requests: Handle,        // where its requests go
    ping: [u8; 16],          // a request that changes nothing
    answers_tx: Handle,      // one channel for all its answers
    answers_rx: Handle,
    patience: u64,           // ticks an answer may take
    next_ping: u64,          // when to ask next
    deadline: Option<u64>,   // while unanswered: when to stop believing
}
```

Each turn of the loop, for each watched service:

1. **If a ping is due and none is outstanding, send one**, and set the deadline to now + patience.
2. **If the deadline has passed, end the process.**
3. When an answer arrives: clear the deadline, schedule the next ping a second later.

### Ending it: one mechanism, reused

Step 2 is one line — `taaj::kill(process)` — and then *nothing else*. No restart code in the watchdog. A killed process ends (entry #17 made sure it does, whether it is spinning, sleeping or waiting); init is already watching for its services to end; and the existing path restarts it:

```
[init] fs has not answered for 4 s - stopping it
[process] 35 "fs" stopped: killed by request
[init] fs ended with code -1001 - restarting it
[fs] ready: TaajFS on the disk, 958 of 1024 blocks free; a cache of 256 blocks
```

The watchdog turns "hung" into "ended". Everything after that was built and tested in entry #10. It also means the restart limit applies unchanged: a service that hangs every time it starts is given up on after five tries in 30 seconds, like one that crashes.

### A full queue is an answer too

If the service's queue is full, the ping cannot be sent. I count that as sent: the deadline starts anyway. A service that leaves its queue full for its whole patience is not answering either — and treating "could not ask" as "fine" would make the watchdog blind in exactly the overload case.

### Any answer means alive

All of a service's answers come back on one long-lived channel, and init does not match answers to pings. A late answer to an old ping still proves the loop is turning. After a restart the channel is drained, so the new process is not credited with the old one's replies.

Why one long-lived channel, when every other client makes a fresh reply channel per request (entry #13)? An honest, slightly embarrassing reason: **the tests count channels.** The boot test compares `ps` before and after a long session and requires the same number, to catch leaks. A channel created and destroyed twice a second would make that number flicker, and the leak check would fail at random a few times in a hundred. So each ping carries a *duplicate handle* to the same channel instead: handles come and go, the number of channels does not.

It is a case of the test shaping the design — but the design it pushed me to is also the cheaper one.

## What the clients see: nothing

Here is the part I did not plan and like most.

init holds both ends of each service's request channel (entry #10), so the channel outlives the process. When the hung `fs` is killed, **the requests waiting in its queue stay there.** The new `fs` mounts the disk, starts its loop, and finds them.

So a client that sent a request to the old, stuck service gets its answer from the new one. It never learns anything happened:

```
taaj> fs stall 6
fs: the file service will answer nothing for 6 s
taaj> ls
                       <- about five seconds: stuck, noticed, replaced
   20920  append
     300  broken
   ...
12 files, 164185 bytes
```

No error. A pause.

For that to work, one number had to change. A client's patience must be **longer than the system's time to recover**, or the client gives up just before help arrives. The file service is replaced at most 5 seconds after it goes silent (a ping at most 1 s later, 4 s of patience). So the clients' limit, `ANSWER_WAIT`, went from 5 seconds to 10.

That relation is the real design rule here:

> **client patience > detection time + restart time.**

Get it backwards and the two mechanisms fight: clients fail, *then* the service recovers.

### Is it safe to kill the file service mid-request?

Yes — and this is entry #15 paying off. Every change is one copy-on-write commit; the disk always holds either the complete old state or the complete new one. A file service killed halfway through a write leaves the old state. And the reply is sent only after the commit, so no client was told "ok" for something that is not on the disk.

One real subtlety: a request that was **being worked on** when the service was killed — not waiting in the queue, but already taken out — is gone. Its client waits out its 10 seconds and is told the service is not answering. And if that request had already committed but not yet replied, the client is told "unknown" for a change that happened. That is the meaning of a timeout (entry #21), and it has not changed.

## Why two different patiences

`blk` gets 2 seconds, `fs` gets 4. Not because one is slower.

`fs` *depends on* `blk`. If the disk driver hangs, the file service — waiting for a sector — goes silent too. With equal patience, both would be declared dead at the same moment and both restarted, the file service for nothing.

With the dependent service more patient, the order comes out right: `blk` is replaced within 3 seconds, its queue (also kept by init) still holds the file service's request, the new driver serves it, and `fs` answers its ping with a second to spare.

> **A service that waits on another must be given longer than it takes to replace the other.**

Patience has to follow the dependency graph. With two services I wrote the numbers by hand. With twenty this wants to be derived.

## What it does not cover

Honest list:

- **`display`, `kbd` and `shell` are not watched.** They do not answer requests — the display only receives, the keyboard driver only sends, the shell talks to a person. There is nothing to ping. Watching them needs each to grow a way to answer, which is a change to each; a hung display or keyboard driver still needs a reboot today.
- **A poisoned request.** If one particular request makes the service hang, the new service finds it in the queue and hangs again — five times, and then init gives up. Dropping the request at the head of the queue after a hang would fix it and would also lose innocent requests. Not decided.
- **Slow is not dead.** A service doing honest work for more than its patience would be killed. Nothing in `fs` or `blk` takes anywhere near that long today — each request is one small operation — but a future operation that does must either be split up or answer pings in between.
- **The `blk` path is not tested.** The mechanism is the same code, but there is no `blk stall`, so the driver's replacement — and the "fs outlasts it" ordering argued above — has not been observed. It is reasoning, not evidence.
- **Who watches init?** Nobody. If init hangs, supervision stops. It is small, waits on nothing, and that is all the protection it has.

## How I checked it

- **Boot tests, BIOS and UEFI.** `fs stall 6`, then `ls`. Required in the log: the stall, `[init] fs has not answered for 4 s - stopping it`, the kernel's `stopped: killed by request`, `[init] fs ended with code -1001 - restarting it` — and then `ls` must print its listing, with no error. Later in the same session the disk is read, written and checked from the host as before: the replaced service is a working one.
- **No false alarms.** The session includes the heaviest things the system does — `cat` of a 300-line file, `disk check`, programs loaded from disk. The watchdog line must appear exactly once per boot. Across five boots: once each.
- **The leak check** still holds: `ps` before and after reports the same counts (now 10 channels: two more, permanently, for the watchdog's answers).
- `make test64`: all passed.

## What I learned

- **Health is answering, not existing** — and must be checked through the front door.
- **Convert the new failure into an old one.** The watchdog does not recover anything. It turns "hung" into "ended", which was already handled.
- **Timeouts in layers must be ordered.** Client patience above recovery time; a dependent service's patience above its dependency's. Otherwise the layers defeat each other.
- **Keeping the queue outside the process** (decided in entry #10 for a different reason) is what makes replacement invisible.
- **Tests shape designs.** Sometimes for the better; worth noticing either way.

---

## Testing note

```
make run64
```

**The watchdog replacing a stalled file service**

| Type | You should see |
|---|---|
| `ps` | Note the process number of `fs` |
| `fs stall 20` | `fs: the file service will answer nothing for 20 s` |
| `ls` (at once) | A pause of about 4–5 seconds — then the **listing**. No error |
| `ps` | `fs` with a **new** process number; everything else unchanged |
| `write t after`, `cat t`, `rm t` | All work: the new service is a full one |

In the terminal log, in this order:

```
[fs] asked to stall: answering nothing for 2000 ticks
[init] fs has not answered for 4 s - stopping it
[process] N "fs" stopped: killed by request
[init] fs ended with code -1001 - restarting it
[fs] ready: TaajFS on the disk, …
[init] started ls for the shell, from a file
```

Note that `[fs] answering again` does **not** appear: the stalled service never woke up. It was replaced.

**No false alarms**

| Do | You should see |
|---|---|
| Use the system normally for a few minutes — `ls`, `cat`, `write`, `time ls`, `disk check` | The line `has not answered` **never** appears in the terminal log. If it ever does without a `fs stall`, that is a bug: tell me what was running |
| `echo`, `uptime`, `mem` during a stall | Immediate, as before |

**Giving up** (optional, and it ends the session's file access): `fs stall 20` six times, each as soon as `ls` works again. After five restarts inside 30 seconds init prints `[init] fs ended (code -1001) 5 times in 30 s - giving up on it`; after that, file commands fail after 10 seconds with `the file service is not answering`. This path has no automated test.

Automated: `make test64`.
