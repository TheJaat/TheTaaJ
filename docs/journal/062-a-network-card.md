# A network card

*Journal entry #62 · Era 4, step A, first half. The system has a network card: a driver (`nic`) for the virtio card on both machines, a small protocol to hand whole Ethernet frames to it and get them from it, and a test with a second computer on the other end of the cable - a program on the host that looks at every frame the machine sends. No protocols yet: this is the wire, and nothing on it.*

```
[pci] 00:04.0 virtio network card: ports 0xc080+0x20, 160 KiB DMA at 0xf24000, MSI-X - handed to init
[init] started: registry display fs blk nic shell kbd; supervising
[nic] virtio network card at 00:04.0: address 52:54:00:12:34:56, link up, queues of 256 and 256, MSI-X interrupts
[niccheck] 233 checks, all passed: card 52:54:00:12:34:56, 37 frames out, 176 in (101 passed on, 2 for others, 73 dropped)
```

## Why

Era 3 ended with the same system on two CPUs. Era 4 is the network: at its end the machine should serve a web page that `curl` on the host can fetch. I planned it as three big steps:

| Step | What |
|---|---|
| A | the card's driver; Ethernet, ARP, IPv4, ICMP (ping), UDP |
| B | TCP, and sockets - also from C |
| C | an HTTP server, a page fetched with `curl` |

This entry is the first half of A: the card. The protocols need a crate of their own that can be tested on the host, and where that crate lives is not decided yet. Everything here needed no new crate.

It is also the right place to stop for a moment. Everything above this layer is arithmetic on bytes, which I can test on the host. This layer is the only one that touches hardware, and the only one where "it works" has to be shown with a real wire.

## What a network card is

A disk and a network card look alike from far away: both move blocks of bytes. They differ in one way, and that way shapes the whole driver.

**A disk only answers. A card also speaks first.**

A disk does nothing until asked. A frame arrives on the wire when somebody out there sends it - asked for or not, now or in an hour. So the card must always have somewhere to put the next frame, *before* it comes.

What travels on the wire is an **Ethernet frame**:

```
 0        6        12    14                          up to 1514
+--------+--------+-----+---------------------------+
|  to    |  from  |type |  payload                  |
+--------+--------+-----+---------------------------+
 6 bytes  6 bytes   2     46 .. 1500 bytes
```

- **to / from** are card addresses (MAC addresses): six bytes that every card has, unique to it. `ff:ff:ff:ff:ff:ff` means "everyone". An address whose first byte is odd means "a group of cards".
- **type** says what the payload is: `0x0800` is IPv4, `0x0806` is ARP. I use `0x88B5` in the test - a number set aside for experiments, so that nothing else on a network takes my test frames for its own.
- A frame on the wire is at least 60 bytes. A shorter one is filled up with zeroes.

The driver knows exactly one thing about a frame: its first six bytes, who it is for. What is inside is the business of whoever listens.

## The virtio card

QEMU's card is a virtio device, like its disk (entries 14 and 18), so most of the machinery was there: the `virtio` crate's queues, the PCI and MMIO discovery in the kernel, DMA memory handed to a driver process.

A virtio network card has two queues:

```
          receive queue (0)                     transmit queue (1)

 driver:  "here are 32 empty buffers"   driver:  "here is a frame, send it"
 card:    fills one per frame,          card:    puts it on the wire,
          hands it back                          hands the buffer back
 driver:  passes the frame on,
          offers the buffer again
```

The receive queue is the answer to "the card speaks first": it is kept full of empty buffers, offered ahead of time.

Each buffer holds one frame with a ten-byte note from virtio in front of it (`virtio::net::HEADER_SIZE`). The note is for features I do not use (checksums done by the card, several frames in one buffer); it is all zeroes on the way out and skipped on the way in.

The driver's DMA memory (160 KiB, physically contiguous, its address known to the driver so it can tell the card):

```
0x00000  the receive queue: descriptors and rings
0x08000  the transmit queue
0x10000  32 receive buffers of 2048 bytes
0x20000  8 transmit buffers of 2048 bytes
```

Of the features the card offers, the driver accepts two, and that choice is a function with a host test (`virtio::net::accepted`):

| Feature | What it gives |
|---|---|
| `MAC` (bit 5) | the card's address can be read from its configuration |
| `STATUS` (bit 16) | so can "is the cable plugged in" |

### One driver, two machines

As `blk` is for the disk, `nic` is one program for both machines. On the PC the card is on the PCI bus and its registers are I/O ports. On the AArch64 `virt` machine it is at a fixed place in memory. The difference is one small enum:

```rust
enum Registers {
    #[cfg(target_arch = "x86_64")]
    Ports { base: u16, config: u16 },
    Memory(*mut u32),
}
```

A Rust note: `#[cfg(target_arch = "x86_64")]` on an enum *variant* means the variant does not exist at all when compiling for another CPU. Every `match` on the enum then has to carry the same attribute on that arm. It keeps the port instructions out of the AArch64 build without a second file.

The kernels changed very little. Each already found virtio devices and handed a disk to init; now each also hands on a network card. On the PC that became a table instead of an `if`:

```rust
fn offered(device: &Device) -> Option<(&'static str, u64)> {
    if device.vendor != virtio::PCI_VENDOR {
        return None;
    }
    match device.device {
        virtio::PCI_DEVICE_BLOCK => Some(("virtio block device", BLOCK_DMA_PAGES)),
        virtio::PCI_DEVICE_NETWORK => Some(("virtio network card", NETWORK_DMA_PAGES)),
        _ => None,
    }
}
```

init starts `nic` as it starts `blk`: with the card's registers, its DMA memory, a channel with the device's description, the channel requests arrive on, and the interrupt. If there is no card, the service is never started and the request channel is closed, so a client is told "no network card" at once instead of waiting.

## Frames between programs: `abi::nic`

A driver is a process. A program that wants to send a frame sends a *message* to it. Here is the first problem: a message holds at most 1024 bytes (`abi::MESSAGE_MAX`), and a frame is up to 1514.

So a frame travels in one or two messages:

```
PART  <up to 1023 bytes>      "the beginning of a frame; more comes"
LAST  <up to 1023 bytes>      "the end of a frame" (or all of a short one)
DROP                          "forget the PARTs so far"
```

Both directions use this. Putting the pieces back together is `abi::nic::Assembler`, and cutting a frame up is `abi::nic::split` - both plain functions on bytes, with host tests.

`DROP` exists because of the gap between the two messages. A channel's queue holds 32 messages. If the queue fills up exactly after a `PART`, the `LAST` cannot be sent. Without `DROP`, the receiver would glue that orphaned first half to the front of the *next* frame. So whoever could not finish a frame remembers it (`torn`), and says `DROP` before starting the next one. Both the driver (to its listener) and the client library (to the driver) do this.

The rest of the protocol:

| Request | With | Answer |
|---|---|---|
| `INFO` | a channel to answer on | the card's address, and whether the link is up |
| `STATS` | a channel to answer on | six counters (below) |
| `LISTEN` | a channel | none: from now on, every frame for this machine goes to that channel |
| `PART` / `LAST` / `DROP` | nothing | none |

Sending has no answer on purpose. A network gives no promise that a frame arrives; an answer from the driver would only say "I gave it to the card", which helps nobody.

The counters make the driver's decisions visible, and the test leans on them:

| Counter | Meaning |
|---|---|
| `received` | frames the card handed over |
| `delivered` | passed on to the listener |
| `not_ours` | for another card or a group: kept back |
| `dropped` | nobody listening, or the listener's queue full |
| `sent` | given to the card |
| `refused` | not a frame: too short, too long, or nonsense |

Every frame received ends in exactly one of `delivered`, `not_ours` or `dropped`, and the test checks that sum.

### Who waits for whom

Two rules, opposite on purpose.

**A listener is never waited for.** If the program that listens does not read, its queue fills, and the driver drops the frame and counts it. The next frame may be for someone who is keeping up, and everything built on a network copes with loss anyway. A driver that waited for one slow program would stop the card for everybody.

**The card is waited for.** The card has eight transmit buffers. While all eight are in its hands, the driver stops reading requests. They stay in the channel; when the channel is full the client's `send` returns `QueueFull`. The program that sends faster than the wire takes is the one that waits - which is what I want TCP to feel later.

```rust
let full = card.transmit_full();
let ready = match card.interrupt {
    Some(line) if full => taaj::object_wait(&[line, line]).map(|_| Some(0)),
    Some(line) => taaj::object_wait(&[line, requests]).map(Some),
    ...
};
```

Normally the driver asks the card *not* to interrupt for sent frames (`Queue::quiet(true)`): it looks for returned buffers whenever it wants one. Only when all eight are out does it ask to be told, and then - the order matters - looks once more before sleeping. A buffer handed back between "none free" and "tell me" would otherwise be one nobody is ever told about.

### The client side

`user/src/nic.rs` is the library half: `Nic::new(channel)`, `info()`, `stats()`, `listen()` which returns a `Frames`, `send(&frame)`, and `Frames::next_until(&mut buffer, deadline)`.

A Rust note: `Frames` closes its channel in `Drop`. `Drop` is the function Rust calls when a value goes out of scope; putting the `close` there means a program cannot forget it, on any path out of a function - including the early `return` of every failed check in the test.

## Testing a card: someone has to be on the other end

A card cannot be tested alone. What it sends has to arrive somewhere, and what it receives has to come from somewhere.

QEMU can connect an emulated card to a pair of UDP ports on the host:

```
-netdev socket,id=n0,udp=127.0.0.1:OURS,localaddr=127.0.0.1:THEIRS
```

Every frame the machine sends arrives at port OURS as one datagram: the raw bytes of the frame. Every datagram sent to THEIRS is a frame the card receives. So a short Python program can be a second computer on the wire. That is `kernel/kernel64/scripts/wire.py`.

```
   TheTaaJ (in QEMU)                          the host
 +-------------------+                  +------------------+
 | niccheck          |                  |  wire.py         |
 |    |  abi::nic    |                  |   sees every     |
 | nic (driver)      |    UDP, one      |   frame, answers |
 |    |  virtio      |    datagram      |   the ones it    |
 | card  ------------+--- per frame ----+-  knows          |
 +-------------------+                  +------------------+
```

`niccheck` (a program on the disk) asks things of the peer with frames of the experiment type; byte 14 says what:

| Asked | The peer does |
|---|---|
| `ECHO` | sends the frame back to this card alone, every data byte flipped (XOR `0x5A`) |
| `BURST n` | sends n numbered frames of 100 bytes at once |
| `LONG n` | the same, of 1514 bytes each |
| `STRANGERS` | sends a frame to another card, one to a group, then one to everyone |

The echo is flipped so that an answer proves bytes went out and *other* bytes came in - not that something inside looped them back.

And the peer checks the machine in turn, because it sees what `niccheck` cannot: the wire itself. Its verdict fails if any frame was shorter than 60 bytes, if frames came from two addresses, or if it ever saw a frame marked `9` - the mark `niccheck` puts in everything the driver is supposed to refuse.

What `niccheck` checks, in order:

1. The card has an address, for one card; the link is up.
2. Frames of every awkward length go out and come back whole: 16, 59, 60, 61, 100, 1022, 1023, 1024, 1500, 1513, 1514 bytes. (59/60/61 are around the wire's minimum; 1022/1023/1024 are around where a frame is cut into two messages.)
3. Those eleven round trips take less than a second (why: see Bugs).
4. What is not a frame is refused and counted, and never reaches the wire.
5. Frames for other cards are kept back, and counted.
6. A burst that is read arrives complete and in order.
7. A burst of 100 that nobody reads: exactly 32 arrive (the queue), 68 are dropped and counted, the driver did not wait.
8. Long frames nobody reads: the queue fills in the middle of one, and the frame after that half arrives clean.
9. The driver kept busy while this program sends until `QueueFull` in the middle of a frame: everything handed over goes out afterwards, the half does not.
10. A second listener takes over; the first is told.
11. The counters add up.

`niccheck restart` is a second test: it asks the driver to do nothing for twenty seconds. init's watchdog must notice (it asks every service a question and expects an answer within two seconds), stop the driver, start another - and the card must work again with the new one.

On any other network nobody answers `niccheck`'s first frame. It then says so and stops, with exit code 77, having sent one harmless frame.

## Design decisions

| Decision | Chosen | Rejected, and why |
|---|---|---|
| Frames bigger than a message | Two messages (`PART`, `LAST`) | Raising `MESSAGE_MAX` to 2048 for the whole system: every channel message buffer in every program would double for the sake of one protocol. A shared memory ring between driver and client: faster, and the right answer one day - but a lot of machinery before a single frame has moved |
| Slow listener | Drop and count | Wait for it: one stuck program would stop the card for all |
| Card has no room to send | Stop reading requests; the client gets `QueueFull` | Refuse the frame silently (my first version): the client cannot tell, and later a TCP would see it as loss on a wire that lost nothing |
| Answer to a sent frame | None | "ok, given to the card": costs a channel per frame and promises nothing useful |
| Who filters by address | The driver, on the first six bytes | The card (virtio has a feature for it): needs a third queue; and QEMU's card hands over everything anyway |
| Group (multicast) frames | Kept back | Passed on: nothing listens for a group yet. This will change with IPv6 |
| How many listeners | One; a new one replaces the old, which is told | Several, each with a filter: that is the protocol stack's job, and it will be the one listener |
| The test's peer | A program on the host, over QEMU's UDP socket backend | QEMU's built-in "user" network: it answers ARP and ping, not arbitrary frames, and cannot report what it saw. A second QEMU machine: twice the boot time, and both ends would be my code - the same mistake on both sides would pass |
| Transmit interrupts | Off, except while every buffer is in use | Always on: an interrupt per frame sent, to learn something the driver finds out anyway when it next wants a buffer |

## Bugs

**The test was wrong before the driver was.** `niccheck` expected the padding of a short frame to come back as zeroes. But the peer flips *every* byte after the header, padding included - so zero padding comes back as `0x5A`. The check now expects that, which is in fact the stronger check: it proves the padding was zeroes on the wire.

**The typed-session test's own expectation.** The shell's `services` command now lists `nic` too, and the boot test compares that line exactly. Updated.

**Interrupts that were never proven.** I removed the two lines that acknowledge the card's interrupt on AArch64, on purpose, to see whether the test would notice. It did not: all checks passed. The driver took one interrupt and never another, and still found every frame - because init's watchdog asks it a question now and then, which wakes it, and then it looks at its queues. Correct, and far too slow. Hence check 3 above: eleven round trips in under a second. With the two lines removed, it now fails.

**Half-frames that were never tested.** Another deliberate fault - telling the listener `DROP` when nothing was torn - changed nothing, which is right (a `DROP` with nothing to forget is harmless). But it showed me that no test ever reached the real case. Checks 8 and 9 are built to hit it exactly: one short frame first, so that the queue's 32nd place falls on the first half of a long one.

**Refusing instead of waiting.** While writing check 9, I saw that my driver, with all eight transmit buffers out, simply counted the frame as refused. On QEMU that never happened - but only because QEMU's card is instant. Replaced by the back-pressure described above.

## How I checked it

**Automated, on the host**

- `abi`: 8 tests of the frame protocol - `split` and `Assembler` at every boundary length, too-long frames, nonsense between pieces, `DROP`, `for_us`, `Stats` to bytes and back.
- `virtio`: 3 new tests - the features accepted, the link bit, `Queue::quiet`.

**Automated, booted** (`make net64-test`, `make aarch64-net-test`, both part of `make test64`)

Each boots the machine with its card wired to `wire.py`, types `niccheck` and then `niccheck restart`, and requires:

- the driver's start line, with an address and "link up";
- `[niccheck] 233 checks, all passed`;
- the peer's own verdict `wire: ok`;
- the address the driver read from the card is the address its frames carry;
- the number of frames `niccheck` counted as sent is the number that arrived at the peer (39 with the restart test's two);
- init noticed the hung driver, stopped it, restarted it; two driver start lines; `niccheck restart: 9 checks, all passed`.

Then each boots again with nobody on the other end and requires "nobody answers" and exit code 77.

**Deliberate faults.** I put 16 faults into the code one at a time and ran the booted test against each:

| Fault | Result |
|---|---|
| short frames not filled up to 60 bytes | caught |
| frames for other cards passed on | caught |
| receive buffers not offered again | caught |
| a dropped frame counted as delivered | caught |
| the old listener not closed | caught |
| transmit buffers never taken back | caught |
| a too-long frame not counted as refused | caught |
| virtio's note taken to be 12 bytes, not 10 | caught |
| no watchdog for the driver in init | caught |
| the length check before sending removed | caught |
| the card's address feature not accepted | caught |
| AArch64: the interrupt never acknowledged | first survived, caught after check 3 |
| the driver never says `DROP` to its listener | caught (by check 8) |
| the client library never says `DROP` | caught (by check 9) |
| transmit interrupts never turned off | **survived** - expected: it only costs time |
| no waiting while all transmit buffers are out | **survived** on both machines |

The last one is the honest gap. QEMU's card hands a buffer back before the driver can send the next frame, so "all eight out" never happens in the test. As an experiment I changed the driver to take buffers back *only* when it found none free, and to say so: the path then ran six times in one test, once with truly nothing free - it slept on the interrupt and woke, and all 233 checks passed. So the code has run and worked once, in an experiment; it is not covered by the tests I keep.

**By hand:** the steps in the testing note below, on both machines.

**Not tested**

- A real card, or any card but QEMU's virtio one.
- The cable being pulled while the system runs: the link state is read once, at start.
- The driver without an interrupt (polling each tick): the code path exists, and no machine I boot takes it.
- The back-pressure path, except in the experiment above.

## What is still missing

- Everything above the wire: ARP, IPv4, ICMP, UDP - the second half of this step. It waits on where the protocol crate lives.
- Only the shell's `niccheck` is given the card. When there is a protocol stack, it will be the one listener, and the card will be taken out of the shell's namespace.
- One card. A second one is found and ignored.
- A frame is copied three times between the card and a program (card buffer → messages → the program's buffer). Fine for a web page; a shared ring is the fix when it matters.
- On AArch64 the disk's and the card's registers lie in the same 4 KiB page, and each driver is given that whole page. So `nic` could write to the disk's registers, and `blk` to the card's. A driver's reach should end at its own device; today, on this machine, it ends at the page.
- The link state is read once. The card can interrupt when it changes; the driver does not ask for that.

## What I learned

- The difference between a device that answers and a device that speaks first decides the whole shape of a driver: buffers offered ahead of time, and a rule for what happens when nobody takes what came.
- "Who waits for whom" should be decided per direction, and written down. Inbound: nobody waits, frames are dropped. Outbound: the sender waits. Getting either backwards gives a system that works in a demo and stalls under load.
- A test passing with a line deleted is information about the test. Three of the faults I planted survived, and each pointed at something real: interrupts never proven, a path never reached, and a silent refusal that should have been a wait.
- Something else in the system can hide a bug. The watchdog's regular question kept a deaf driver alive. Correct results are not enough for a driver; *when* matters too.
- To test one end of a wire, the other end should not be my own code running the same way. A peer written separately, in another language, that reports what it saw, catches what a loopback never would.
- An emulator is kinder than hardware. QEMU's card never runs out of buffers and never loses a frame; the paths for those cases have to be forced, or admitted as untested.

## Testing note

**1. The card, on the PC**

```
make run64
```

`make run64` opens a QEMU window to type in; the kernel's log appears in the terminal it was started from. The machine now has a network card, on QEMU's own small network.

In the terminal's log:

| Look for | Meaning |
|---|---|
| `[pci] 00:04.0 virtio network card: ports 0x…, 160 KiB DMA at 0x…, MSI-X - handed to init` | the kernel found the card and gave it to init |
| `[init] started: registry display fs blk nic shell kbd; supervising` | `nic` is one of the services |
| `[nic] virtio network card at 00:04.0: address 52:54:00:12:34:56, link up, queues of 256 and 256, MSI-X interrupts` | the driver is running |

In the QEMU window:

| Type | You should see |
|---|---|
| `services` | `keys display init system files files.look disk.look nic` |
| `niccheck` | `[niccheck] nobody answers on this network: the card was asked to send one frame, and nothing more`, then `niccheck exited with code 77` - right: on this network nobody speaks the test's language |
| `niccheck now` | `usage: niccheck, niccheck restart` |

**2. A machine without a card**

```
make run64 RUN_NET=
```

| Look for / Type | You should see |
|---|---|
| in the log | no `[nic]` line; `[init] started: registry display fs blk shell kbd; supervising` |
| `niccheck` | `[niccheck] FAILED: no network card (or its driver stopped)` |

**3. With someone on the other end of the wire**

In one terminal, start the peer; it writes its two port numbers to a file:

```
kernel/kernel64/scripts/wire.py /tmp/wire.ports /tmp/wire.verdict &
cat /tmp/wire.ports
```

That prints two numbers, for example `46011 53872`. Put the first after `udp=127.0.0.1:` and the second after `localaddr=127.0.0.1:`:

```
make run64 RUN_NET="-netdev socket,id=n0,udp=127.0.0.1:46011,localaddr=127.0.0.1:53872 -device virtio-net-pci,netdev=n0"
```

| Type | You should see |
|---|---|
| `niccheck` | `[niccheck] 233 checks, all passed: card 52:54:00:12:34:56, 37 frames out, 176 in (101 passed on, 2 for others, 73 dropped)` (a few seconds; the log also shows `[nic] asked to stall: doing nothing for 50 ticks` and `[nic] working again` - that is check 9 keeping the driver busy) |
| `niccheck restart` | in the log: `[nic] asked to stall: doing nothing for 2000 ticks`, then after two seconds `[init] nic has not answered for 2 s - stopping it`, `[init] nic ended with code -1001 - restarting it`, a second `[nic] virtio network card at …` line, and `[niccheck] restart: 9 checks, all passed: the driver was stopped, another started, and the card works again` |

Close QEMU, then ask the peer what it saw:

```
kill %1; sleep 1; cat /tmp/wire.verdict
```

| Look for | Meaning |
|---|---|
| `wire: ok: 39 frames from 52:54:00:12:34:56: 34 to echo, 4 asking for a burst, 1 for frames to others` | every frame was at least 60 bytes, all from one address, none that should have been refused |

**4. The same on AArch64**

```
make run-aarch64
```

The terminal is the machine's keyboard and screen. To leave QEMU: **Ctrl+A, then X**.

| Look for | Meaning |
|---|---|
| `[virtio] 0xa003c00: network card (legacy interface), interrupt 78 (edge)` | found in the device tree's list of places |
| `[virtio] 0xa003c00 network card: handed to init` | given to init |
| `[nic] virtio network card at 0xa003c00: address 52:54:00:12:34:56, link up, queues of 1024 and 1024, interrupts` | the same driver, its registers in memory this time |

| Type | You should see |
|---|---|
| `niccheck` | `[niccheck] nobody answers on this network: …` and `niccheck exited with code 77` |

With the peer: start `wire.py` again as in step 3 (it was stopped there), and use the two numbers it writes this time:

```
make run-aarch64 RUN_A64="-netdev socket,id=n1,udp=127.0.0.1:46011,localaddr=127.0.0.1:53872 -device virtio-net-device,netdev=n1" RUN_A64_DEVICES="-drive file=build/aarch64/taaj-disk.img,if=none,format=raw,id=d0 -device virtio-blk-device,drive=d0"
```

| Type | You should see |
|---|---|
| `niccheck` | `[niccheck] 233 checks, all passed: card 52:54:00:12:34:56, 37 frames out, 176 in (101 passed on, 2 for others, 73 dropped)` |

**5. The tests by themselves**

```
make net64-test
make aarch64-net-test
```

Each ends with a `wire: ok: 39 frames from …` line and `net64-test passed` / `aarch64-net-test passed`. The logs are `build/net64.log` and `build/aarch64/net.log`; the peer's verdicts are beside them (`.wire`).

**6. Host checks**

```
cd kernel/abi && cargo test nic
cd virtio && cargo test
```

`8 passed` and `11 passed`.

Automated: `make test64`
