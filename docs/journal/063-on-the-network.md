# On the network

*Journal entry #63 · Era 4, step A, second half. The machine has an address. A new crate, `net`, holds the protocols - Ethernet, ARP, IPv4, ICMP, UDP - as plain arithmetic on bytes, tested on the host. A new service, also called `net`, owns the card's frames and gives programs the network: `ping`, and UDP ports. It is tested against two other implementations: a router I wrote by hand in Python, and QEMU's own network stack.*

```
[net] this machine is 10.0.2.15/24 on card 52:54:00:12:34:56, with its router at 10.0.2.2
> ping 10.0.2.2 2
56 bytes from 10.0.2.2: seq=1 ttl=255 time 10 ms
56 bytes from 10.0.2.2: seq=2 ttl=255 time under 10 ms
2 sent to 10.0.2.2, 2 answered
> udpsend 10.0.2.2 4000 hello from taaj
udpsend: 15 bytes from port 49152 to 10.0.2.2:4000
udpsend: 21 bytes back from 10.0.2.2:4000: pong: hello from taaj
```

## Why

Entry 62 ended with a card that moves frames and nothing that knows what is in one. This entry is everything between a frame and a program: the second half of step A of Era 4. After it, the machine answers pings and exchanges datagrams with other machines. TCP (step B) and a web server (step C) are built on exactly this.

## The layers

Each protocol is the payload of the one before it:

```
+-----------------------------------------------------------------+
| Ethernet: to card, from card, type                              |
|  +------------------------------------------------------------+ |
|  | IPv4: from machine, to machine, protocol, checksum         | |
|  |  +-------------------------------------------------------+ | |
|  |  | UDP: from port, to port, length, checksum             | | |
|  |  |  +--------------------------------------------------+ | | |
|  |  |  | data                                             | | | |
|  |  |  +--------------------------------------------------+ | | |
|  |  +-------------------------------------------------------+ | |
|  +------------------------------------------------------------+ |
+-----------------------------------------------------------------+
```

| Layer | Its question | Its address |
|---|---|---|
| Ethernet | which card on this wire? | 6 bytes, `52:54:00:12:34:56` |
| IPv4 | which machine, anywhere? | 4 bytes, `10.0.2.15` |
| UDP | which program on that machine? | a port, `7` |

Two more protocols sit beside these:

- **ARP** joins the first two layers. IPv4 knows it wants `10.0.2.2`; Ethernet needs a card address. ARP asks every card on the wire: "who has 10.0.2.2?", and the one that has it answers.
- **ICMP** is the network's own messages, carried inside IPv4: "are you there?" (ping), "nobody has that port".

## The `net` crate: protocols with no machine

The crate has three parts, and none of them touches a card, a clock or a channel:

| File | What |
|---|---|
| `checksum.rs` | the one checksum IPv4, ICMP and UDP all use |
| `wire.rs` | each header: read from bytes, written to bytes |
| `stack.rs` | what a machine does with them |

### The checksum

One function, used three times. Take the bytes two at a time as 16-bit numbers, add them up, add whatever overflowed 16 bits back in at the bottom, invert:

```rust
pub fn add(mut sum: u32, data: &[u8]) -> u32 {
    let (words, rest) = data.as_chunks::<2>();
    for word in words {
        sum += u32::from(u16::from_be_bytes(*word));
    }
    if let [last] = rest {
        sum += u32::from(*last) << 8;
    }
    sum
}

pub fn finish(mut sum: u32) -> u16 {
    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !(sum as u16)
}
```

The neat property: a receiver adds up the same bytes *with* the checksum in place and must get zero. So "is this packet whole?" is `checksum::of(bytes) == 0`.

Two Rust notes. `data.as_chunks::<2>()` splits a slice into a slice of `[u8; 2]` arrays plus the leftover - the sizes are known to the compiler, so there is no index that could be out of range. And `from_be_bytes`: numbers on a network are **big-endian**, most significant byte first, whatever the CPU. Every number read from or written to a packet goes through `from_be_bytes` / `to_be_bytes`.

### Reading headers: never believe a length

A packet says how long it is, in several places, and any of them can lie. The rule in `wire.rs` is that every stated length is checked against the bytes that are really there before anything is sliced:

```rust
let header_length = usize::from(packet[0] & 0xF) * 4;
let total = usize::from(be16(packet, 2));
if header_length < HEADER || total < header_length || total > packet.len() {
    return Err(Bad::Length);
}
if checksum::of(&packet[..header_length]) != 0 {
    return Err(Bad::Checksum);
}
```

There is also a subtle point about frames. A frame is at least 60 bytes, so a short packet arrives with zeroes after it. The IPv4 `total` length says where the packet really ends; everything after is the frame's filling, and must not be taken for data.

UDP's checksum has one more oddity: it also covers the two IP addresses, which are not in UDP's own header (a "pseudo-header"). So a datagram that arrives at the wrong machine fails its checksum. And a checksum of zero means "the sender made none", so a sum that comes out as zero is sent as `0xFFFF` - which adds up to the same.

### The stack: handed frames and the time

`Stack` is the part that behaves like a machine. Its whole interface:

```rust
stack.receive(frame, now, &mut wire) -> Received   // a frame from the card
stack.send_udp(from_port, to, to_port, data, now, &mut wire) -> Sent
stack.send_echo(to, identifier, sequence, data, now, &mut wire) -> Sent
stack.tick(now, &mut wire, &mut gave_up)           // the clock
```

`wire` is anything that takes frames to send - in the service, the card's driver; in a test, a `Vec`. `now` is a number of milliseconds, only ever compared. So a host test plays the entire network: it hands in frames, moves the clock, and looks at what came out.

A Rust note: `Wire` is a **trait** - a list of functions a type promises to have - with one function, `send`. `&mut dyn Wire` means "a reference to *something* that has it, decided at run time". That one parameter is what lets the same code run against a real card and against a `Vec<Vec<u8>>`.

What the stack does by itself:

**It answers "who has my address?"** - to the asker alone - and remembers who asked, because an answer to them will probably be wanted next.

**It asks before it sends.** To send to `10.0.2.2` it needs that machine's card address. If it does not know it:

```
send_udp(... to 10.0.2.2 ...)
   |
   +-- card address known?  yes -> frame goes out            Sent::Sent
   |
   no
   +-- the packet is held back (at most 4 at once)           Sent::Waiting
   +-- "who has 10.0.2.2?" goes to every card
          |
          +-- answer comes  -> every packet held for it goes out, in order
          +-- no answer     -> asked again after 1 s, and again;
                               after 3 s the packets are dropped
```

Answers are believed for one minute, then asked for again: cards get replaced and addresses move.

**It decides where a packet goes.** The prefix (`/24`) says which addresses are neighbours on this wire. A packet for a neighbour is handed to the neighbour's own card. A packet for anywhere else is handed to the router's card - but with the far machine's address still in the IPv4 header. That distinction, *frame to the router, packet to the destination*, is all that routing is at this end.

**It answers pings**, with the same bytes back.

**It drops, and says why.** Wrong checksum, wrong lengths, for another machine, a fragment (a piece of a packet that was cut up on the way - nothing here puts pieces together), a protocol not spoken. Each has a counter.

Some things it refuses on purpose:

- A ping sent to *everyone* is not answered. If every machine answered, one forged packet would make a whole network shout at one address.
- A claim that another card has *this machine's* address is not believed.
- A card address is learned only from ARP, and only when the packet was addressed to this machine or the address was being asked for. Otherwise the table of sixteen would fill with everyone who ever asked anything.

## The `net` service

`user/src/bin/net.rs` is the process around the stack - the plumbing:

```
        programs: ping, udpecho, udpsend, netinfo
             |  requests (abi::net)        ^ |  one channel per socket
             v                             | v
        +--------------------------------------------+
        |  net   table of ports, pings in flight     |
        |        Stack (the `net` crate)             |
        +--------------------------------------------+
             ^ frames of type IPv4 and ARP   | frames to send
             |                               v
        +--------------------------------------------+
        |  nic   the card's driver (entry 62)        |
        +--------------------------------------------+
```

init starts it after `nic`, gives it a channel to the registry on which exactly one name exists - `nic` - and watches it like every other service. Its address is fixed for now: `10.0.2.15/24`, router `10.0.2.2`, which is what QEMU's network gives a machine.

### A socket is a channel

A program asks for a UDP port with `UDP_BIND`, and hands over one end of a channel with the request. From then on **that channel is the socket**: each message on it is one datagram - `[address][port][data]` - in both directions.

The reason is what happens when a program dies. A channel closes when its last holder goes away, however it went. The service sees the socket's channel close and frees the port. There is no "close socket" request to forget, and no table of who owns what to clean up.

The same idea gives permission for free: only whoever holds the channel can send from that port.

### One card, two listeners

In entry 62 the driver had one listener, and I wrote that the stack would become that one. That would have ended `niccheck`, the driver's own test, which needs the card too.

So a listener now names the **frame types** it wants (`abi::nic::Listeners`). `net` asks for `0x0800` (IPv4) and `0x0806` (ARP); `niccheck` asks for its experiment type `0x88B5`. The driver looks at two more bytes of each frame and hands it to whoever named its type. A new listener takes over the types it names from whoever had them, and a listener left with none is closed.

It is the same decision an operating system makes everywhere on the way up: Ethernet type → IP protocol → port. Each layer reads one small number and picks who is next.

### Ping, from a program's side

`ping` sends one request to the service and gets one answer: who replied and with what "time to live", or `TIMED_OUT`, or `UNREACHABLE` (nobody on the wire has that address). The service keeps the ping in a small table until its echo comes back, matched by an identifier it chose itself and by every byte of the data.

## Testing against something that is not mine

The host tests are thorough, but they have a weakness: the test and the code were written by the same person with the same understanding of the standards. If I misread how a checksum is folded, both sides would agree with each other and with nobody else.

So the booted tests use two other implementations.

**A router made by hand** (`kernel/kernel64/scripts/wire-ip.py`). As in entry 62, QEMU hands every frame the machine sends to a UDP port on the host. This program plays `10.0.2.2`: every header is packed and unpacked with Python's `struct`, every checksum worked out there, not one line shared with the system. It does two things at once:

| It asks the machine | And expects |
|---|---|
| who has 10.0.2.15? | the card's answer, to the asker alone |
| pings of 0, 56 and 1472 bytes | each back, byte for byte |
| a ping with a wrong ICMP checksum | silence |
| a packet with a wrong IPv4 checksum | silence |
| a ping for 10.0.2.16, in a frame for this card | silence |
| a ping to everyone | silence |
| a piece of a cut-up packet | silence |
| a datagram to port 9, which nobody has | "port unreachable", quoting the datagram |
| datagrams of 1, 100 and 1018 bytes to port 7 | each echoed, from port 7, checksum right |
| a datagram of 1200 bytes | silence |

And it answers what the machine asks: pings to `10.0.2.2`, a datagram to port 4000. It checks every frame the machine sends on the way: frame length, the time to live, both checksums, that ARP questions go to everyone.

Then the machine is asked what *it* counted (`netinfo`), and the numbers must match what the router sent exactly:

```
answered 1 questions for this address, 3 pings
datagrams 7 in; 1 for a port no program has, 1 a program did not take
dropped  2 damaged, 1 for others, 2 of a kind not spoken here
```

So every bad packet was not only left unanswered, it was dropped for the right reason.

**QEMU's own network** (`udp-host.py`). QEMU's "user" network is a complete small network stack inside the emulator, written by other people long ago. On this one the other side is ordinary programs with ordinary sockets on the host. If a datagram sent from a host socket arrives in `udpecho` and its echo arrives back in the host socket, my headers were good enough for a stack that is not mine - in both directions.

The typed session is the same in both boots, and ends with the card's driver being made to hang (`niccheck stall`). init replaces the driver; the network service must notice that its frames stopped coming, ask the new driver, and a ping must work again.

## Design decisions

| Decision | Chosen | Rejected, and why |
|---|---|---|
| Where the protocols live | A crate with no I/O; the service is plumbing | In the service: nothing could be tested without booting |
| Time in the stack | A number passed in | Reading a clock inside: a test of "asked again after one second" would take a second, and three for giving up |
| A socket | A channel, both ways | A port number in every request: anyone could send from anyone's port, and a dead program's ports would stay taken |
| Datagram size | At most 1018 bytes of data (what one message carries); longer ones arriving are dropped and counted | Two messages per datagram, as for frames: it can come later; DNS and DHCP fit in 1018 |
| Sharing the card | Listeners by frame type | One listener (entry 62's plan): the driver's own test would have had to go |
| Sender of a datagram is told | Nothing | An answer per datagram: UDP promises nothing, and a program that wants to know builds that itself |
| Unknown port | ICMP "port unreachable" | Silence: the sender waits for nothing, and tools that probe ports rely on the message |
| Ping to everyone | Not answered | Answered: it turns a network into an amplifier |
| Card address learned from | ARP only | Any IPv4 packet's source: one forged packet would redirect traffic |
| Address of this machine | Fixed: 10.0.2.15/24 | DHCP: it is UDP plus a state machine, and belongs after this works |
| The peers in the tests | A hand-written one, and QEMU's | Only host tests: the same misunderstanding on both sides passes |

## Bugs

**init wedged by a long answer.** The first time `ping` worked, the keyboard driver and the network service were both killed by init a few seconds later:

```
2 sent to 10.0.2.2, 2 answered
[init] kbd has not answered for 2 s - stopping it
[init] net has not answered for 4 s - stopping it
```

Neither had stopped. init's watchdog asks each service a small question and reads the answer into a buffer of 16 bytes. The network service's answer to "who is this machine?" is 104 bytes. A message that does not fit is not taken out of its queue - so that channel was "ready" for ever, init looked at nothing else again, never saw the keyboard driver's answers, and concluded that both were dead. The buffer is now as long as the longest message there is. The lesson is about the supervisor, not the service: one answer of the wrong size must never be able to blind it.

**My hand arithmetic.** The reference packet in the tests - a ping written out byte by byte - first had two checksums I had worked out by hand, and both were wrong. The code disagreed with them and the code was right. The checksums in the test now come from a separate few lines of Python, and the comment says so. A reference value is only worth something if it does not come from the code under test *and* is not from the same tired head.

**The test started before the machine did.** The hand-made router waits for the machine to say, in its serial log, that the network service is running. On the second run it read the *previous* run's log, found the line at once, and asked "who has 10.0.2.15?" of a machine that had not booted. The script now removes the old log first.

**And gave up before the machine did.** The same waiting had a limit of sixty seconds. Alone, each test boots in a few. In the full suite a boot waits for a free CPU first, and that once took longer than the minute: the peer on the host gave up on a machine that was still queueing. The peers now wait until they are told to stop; the boot script has the only time limit.

**"No answer" for an address nobody has.** A ping waits one second; the stack asks for a card address for three. So a ping to `10.0.2.99` timed out before the stack gave up, and said "no answer" - true, and less than was known. When a ping's time is up and the address it was handed to has never been heard of, it now says "cannot be reached".

**A race I reasoned out and did not see.** `LISTEN` to the driver has no reply. So when the service printed "running", the driver might not yet have read the request, and a frame arriving in that gap would find no listener. `Nic::listen` now follows the request with a question: the driver reads requests in order, so when the answer is back, it is listening. I never saw this fail, and removing the fix does not make a test fail either.

## How I checked it

**Automated, on the host**

- `net`: 30 tests. Headers against bytes written out by hand; every one-bit change to a header or a checksummed payload is rejected; lengths that lie; the ARP exchanges; packets held and released in order; three questions and then giving up, on a clock the test moves; the gateway; expiry after a minute; a full table; and one test that feeds the stack every beginning of some real frames and every single-byte change of them, and requires an answer and not a panic.
- `abi`: the listener table (who gets a type, taking over, no room), and the encodings of `abi::net`.

**Automated, booted** (`make ip64-test`, `make aarch64-ip-test`; both in `make test64`): two boots each, as described above. The checks, in the logs of both: pings to the router answered; a datagram out and its answer back; an address nobody has "cannot be reached"; the echo of 1018 bytes; a taken port refused; the port of an ended program free again; the driver replaced and a ping answered after it; the network service itself never restarted; no service but `nic` ever stopped answering. Plus each peer's own verdict, and with the hand-made router the exact counters.

**Deliberate faults**

| Where | Planted | Caught |
|---|---|---|
| `net` crate (host tests) | 24 | 22 at first. The two that survived - a second carry in the checksum, and held packets not let go - each got a test, and are caught now |
| `abi` (host tests) | 4 | 3 at first; the fourth (the size limit of a datagram message) got a test |
| the service, booted | 7 | 6 |

The one that survives when booted is the `listen` round trip from the last bug above.

**By hand:** the testing note below.

**Not tested**

- Any network but QEMU's. No real card, no real router, no packet that crossed a real wire.
- The network service being restarted: its sockets should close and their programs be told. Nothing makes it hang, so nothing tests it.
- A program that does not read its socket: its datagrams should be dropped and counted. The path exists; no test fills a socket.
- The hand-made router never has to answer "who has 10.0.2.2?": it asks first, and the machine remembers. The machine's own question is checked for form there (it asks about 10.0.2.99), and answered for real only by QEMU's network.
- Sixteen sockets at once, eight pings at once: the limits are code, not tests.

## What is still missing

- **TCP.** Step B.
- **DHCP.** The address is a constant. On any network that is not 10.0.2.0/24 this machine is nobody.
- **DNS.** Addresses are numbers.
- **Fragments.** Not sent (every packet says "do not fragment") and not put together; a datagram bigger than a frame cannot be received at all.
- **Datagrams over 1018 bytes** are dropped on arrival and refused on sending.
- **This machine's own address**: nothing can be sent to it from itself. No loopback.
- **IPv6.** Not a line.
- A program is not told when its datagram could not be delivered ("port unreachable" *arriving* is dropped as unsupported).
- The driver's and the service's log lines aside, there is no way to watch packets - no `tcpdump`.

## What I learned

- A protocol stack is mostly *checking*. The happy path of UDP is thirty lines; the rest is lengths that lie, sums that do not match, packets for someone else.
- Layering is real and visible in the bytes: each layer reads one small number - frame type, protocol, port - and picks who is next. The driver's listeners, the stack's `match`, and the service's port table are the same decision three times.
- Passing time in, instead of reading a clock, turns "wait three seconds and give up" into a test that runs in microseconds.
- Routing at an end machine is one sentence: the frame goes to the router's card, the packet keeps the destination's address.
- "Tested" needs a second opinion. My own tests agreed with my own code about two wrong checksums until a different calculation disagreed.
- Using the kernel object's lifetime as the resource's lifetime (a socket *is* a channel) removes a whole class of cleanup code.
- A supervisor must be more robust than what it supervises. The worst bug of this entry was not in the network at all.

## Testing note

**1. The machine on its network (PC)**

```
make run64
```

A QEMU window opens to type in; the kernel's log appears in the terminal.

In the terminal's log:

| Look for | Meaning |
|---|---|
| `[registry] net may ask for: nic` | the network service may use the card's driver, and nothing else |
| `[init] started: registry display fs blk nic net shell kbd; supervising` | `net` is a service |
| `[net] this machine is 10.0.2.15/24 on card 52:54:00:12:34:56, with its router at 10.0.2.2` | it is running |

| Type | You should see |
|---|---|
| `netinfo` | `address  10.0.2.15/24`, `router   10.0.2.2`, `card     52:54:00:12:34:56`, then four lines of counters, all 0 on a fresh boot |
| `ping 10.0.2.2` | four lines `56 bytes from 10.0.2.2: seq=1 ttl=255 time …` (seq 1 to 4, a second apart), then `4 sent to 10.0.2.2, 4 answered` |
| `ping 10.0.2.2 1 1000` | `1000 bytes from 10.0.2.2: seq=1 ttl=255 time …` |
| `ping 10.0.2.99 1` | after a second: `10.0.2.99 cannot be reached: seq=1, nobody on this network has that address`, `1 sent to 10.0.2.99, 0 answered`, `ping exited with code 1` |
| `ping` | `usage: ping ADDRESS [COUNT [BYTES]] - an address like 10.0.2.2, at most 1472 bytes` |
| `netinfo` | `frames` now counts some in and out |

(10.0.2.2 is QEMU's own router, inside the emulator. Please do not ping addresses outside 10.0.2.x in a test: QEMU would pass those on to the real network.)

**2. Datagrams between the machine and the host**

Start the machine with one of the host's ports passed on to the machine's port 7:

```
make run64 RUN_NET="-netdev user,id=n0,hostfwd=udp:127.0.0.1:5007-:7 -device virtio-net-pci,netdev=n0"
```

| Type (in QEMU) | You should see |
|---|---|
| `udpecho &` | `[1] udpecho`, and `udpecho: listening on port 7` |

Then in another terminal on the host:

```
echo hello | nc -u -w1 127.0.0.1 5007
```

| Look for | Meaning |
|---|---|
| `hello` printed by `nc` | the datagram went in, and its echo came back |
| in QEMU: `udpecho: 6 bytes from 10.0.2.2:…` | the machine saw it (6: `hello` and the newline) |

And the other way. On the host:

```
nc -u -l 127.0.0.1 5008
```

| Type (in QEMU) | You should see |
|---|---|
| `udpsend 10.0.2.2 5008 hello from taaj` | `udpsend: 15 bytes from port 49152 to 10.0.2.2:5008`, and on the host `nc` prints `hello from taaj`. Two seconds later the machine says `udpsend: no answer` (nobody typed one) |
| `udpecho 7 1` | `udpecho: port 7: another program has that port` |

**3. A machine without a card**

```
make run64 RUN_NET=
```

| Type | You should see |
|---|---|
| `ping 10.0.2.2 1` | `ping: no network (no card, or its service stopped)` |

**4. The same on AArch64**

```
make run-aarch64
```

The terminal is the keyboard and screen. To leave QEMU: **Ctrl+A, then X**.

| Look for | Meaning |
|---|---|
| `[net] this machine is 10.0.2.15/24 on card 52:54:00:12:34:56, with its router at 10.0.2.2` | the same service |

| Type | You should see |
|---|---|
| `ping 10.0.2.2 2` | two lines `56 bytes from 10.0.2.2: seq=… ttl=255 time …`, then `2 sent to 10.0.2.2, 2 answered` |
| `ping 10.0.2.99 1` | `10.0.2.99 cannot be reached: seq=1, nobody on this network has that address` |

**5. The tests by themselves**

```
make ip64-test
make aarch64-ip-test
```

Each prints a line from each of the two other sides, and ends `ip64-test passed` / `aarch64-ip-test passed`:

| Look for | Meaning |
|---|---|
| `wire: ok: 15 checks; … frames from 52:54:00:12:34:56; answered 0 questions for 10.0.2.2, 3 pings and 1 datagrams; … questions about addresses nobody has` | the router made by hand found nothing wrong |
| `host: ok: 5 checks; 1 datagrams from the machine answered` | over QEMU's network, datagrams got through both ways |

The logs are `build/ip64.log` (hand-made router) and `build/ip64.log.user` (QEMU's network), and the same under `build/aarch64/ip.log`.

**6. Host checks**

```
cd net && cargo test
cd kernel/abi && cargo test
```

`30 passed` and `21 passed`.

Automated: `make test64`
