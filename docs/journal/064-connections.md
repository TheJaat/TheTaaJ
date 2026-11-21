# Connections

*Journal entry #64 · Era 4, step B. TCP: a connection that delivers every byte, once, in order, or says it failed. It is a module in the `net` crate with no machine in it, tested on the host against a simulated network that loses, doubles and delays segments. The network service hands each connection to its program as a channel, and the program's own speed travels back to the other machine as TCP's window. And C programs get the Berkeley socket calls: `socket`, `connect`, `bind`, `listen`, `accept` - the same source compiles on Linux.*

```
> tcpsend 10.0.2.2 47200 fill 300000
tcpsend: connected to 10.0.2.2:47200 from port 49153
tcpsend: 300000 bytes sent, sum 836cc8b9
tcpsend: 300000 bytes back, sum 836cc8b9, in 380 ms
> cnet send 10.0.2.2 47200 hello from c
cnet: connected to 10.0.2.2:47200 from port 49154
hello from c
cnet: 13 bytes back, and the other side is done
```

## Why

After entry 63 the machine could send datagrams: single packets that may be lost, doubled, or arrive in another order, with nobody told. Almost nothing people use the network for works like that. A web page, a file, a login: each needs a *stream* - bytes that arrive all of them, once, in the order they were sent. That is TCP. Step B of Era 4 is TCP and a way for programs to use it, from Rust and from C. Step C (a web server) is a program on top of exactly this.

## The idea: every byte has a number

TCP makes a reliable stream out of unreliable packets with one idea. Each side numbers the bytes it sends, starting from a number of its own choosing, and every segment says the number of its first byte. Every segment also *acknowledges*: "the next byte I expect from you is number N" - which says everything before N has arrived.

```
  machine (10.0.2.15:49153)                    host (10.0.2.2:47200)
         |  SYN  seq=1000                            |   "my numbering starts at 1000"
         |------------------------------------------>|
         |  SYN+ACK  seq=5000 ack=1001               |   "mine at 5000; I expect your 1001"
         |<------------------------------------------|
         |  ACK  seq=1001 ack=5001                   |   both know both numberings:
         |------------------------------------------>|   the connection is made
         |  seq=1001 "hello\n" (6 bytes)             |
         |------------------------------------------>|
         |  ACK  ack=1007                            |   "everything before 1007 arrived"
         |<------------------------------------------|
         |  FIN  seq=1007                            |   "I have no more to send"
         |------------------------------------------>|
         |  ...                                      |
```

Everything else follows from the numbers:

- **Lost**: what is not acknowledged in time is sent again. Its numbers are the same, so the receiver puts it where it belongs.
- **Doubled**: a segment whose numbers have been passed already is dropped (and answered, because the first answer may be what was lost).
- **Out of order**: a segment that starts after a gap is kept to one side until the gap is filled.
- **Too fast**: each side says, in every segment, how many more bytes it has room for (the *window*). The other sends no more than that.
- **Done**: SYN and FIN take a number each, like a byte, so they are acknowledged and sent again like bytes. Each direction is closed by itself: after my FIN the other side can still send to me.

The numbers are 32 bits and go round: after 4294967295 comes 0. So "a is before b" cannot be `a < b`. It is "b - a, computed modulo 2³², is less than half the circle":

```rust
fn before(a: u32, b: u32) -> bool {
    (a.wrapping_sub(b) as i32) < 0
}
```

`wrapping_sub` is Rust's way of saying "I mean the arithmetic that goes round" - plain `-` would stop the program in a debug build when the result would be negative. Reading the difference as an `i32` makes the upper half of the circle negative: "behind".

## The connection: a machine handed segments and the time

`net/src/tcp.rs` is one connection, and nothing else. Like the rest of the crate it touches no card, no clock and no channel:

```rust
let mut connection = Connection::connect(first_number, Buffers::default());
connection.poll(now, &mut payload)   // -> Some(Out { sequence, acknowledgment, flags, window, .. })
connection.segment(&segment, now)    // a segment arrived for it
connection.tick(now)                 // the clock
connection.write(data)               // the program's bytes, as many as there is room for
connection.read(&mut out)            // what arrived, in order
connection.shutdown()                // "no more from me"
```

`poll` is how anything leaves: called until it says `None`, after anything that may have changed what there is to send. That shape is what makes the whole thing testable: a host test is two connections and a loop that carries `poll`'s output from one to the other's `segment` - or drops it, or carries it twice, or later.

The states have RFC 9293's names:

```
  connect()                         a SYN arrives for a listener
      | send SYN                        | send SYN+ACK
  SynSent                           SynReceived
      | SYN+ACK arrives, send ACK       | its ACK arrives
      +----------> Established <--------+
          shutdown() |        | FIN arrives
            send FIN |        |
              FinWait1        CloseWait
    our FIN is  |    | FIN        | shutdown(): send FIN
  acknowledged  |    | arrives    |
         FinWait2   Closing     LastAck
    FIN arrives |    | our FIN    | our FIN is acknowledged
                |    | is acked   |
               TimeWait         Closed
                   | ten seconds
                Closed
```

TimeWait is the odd one. The side that closed first remembers the connection for a while after it is over, because its last ACK may be lost - and then the other side sends its FIN again, and someone must answer it. Here the wait is ten seconds; the standard asks for minutes, which matters on the Internet, where a segment can wander for that long.

### Sending again, and how long to wait first

What is not acknowledged is sent again when a timer runs out. How long the timer runs is the interesting part. Too short, and a slow answer is taken for a lost one and everything is sent twice. Too long, and a lost segment holds up the connection. So the connection measures: it times one segment at a time, from sending to its acknowledgment, and keeps a smoothed average and how much the times vary (RFC 6298):

```rust
self.variation = (3 * self.variation + smoothed.abs_diff(took)) / 4;
self.smoothed = Some((7 * smoothed + took) / 8);
self.rto = (smoothed + (4 * self.variation).max(10)).clamp(RTO_MIN_MS, RTO_MAX_MS);
```

Two rules keep the measurement honest. A segment that was sent twice is never timed: when its acknowledgment comes, nobody knows which copy it answers (Karn's rule). And after a timeout the wait doubles, again and again, until an acknowledgment comes - then it is the measured one again.

A connection gives up after six timeouts in a row with no word from the other side (a SYN after four: 1 + 2 + 4 + 8 + 16 seconds, 31 in all), and sends a RST in case the other side is still there and only its answers are being lost.

Waiting for the timer is slow when only one segment in the middle was lost. The receiver then answers every later segment with the *same* acknowledgment - the number of the missing byte. Three of those in a row, and the sender sends the missing segment at once ("fast retransmit").

### Holding back: two windows

There are two reasons not to send everything at once, and two limits:

- the other side's room (the window it offers in every segment), which protects the *receiver*;
- this side's own estimate of what the *network* carries (the congestion window). It starts at ten segments, grows by one segment for each segment acknowledged (doubling every round trip: "slow start", which is the fast part), then more slowly. After a timeout it starts again from one segment; after a fast retransmit it is cut to half of what was on its way. A lost packet usually means a router on the way is full, and sending as before would make it worse for everyone.

When the other side's window is shut and stays shut, the connection asks now and then with one byte past the window ("are you open yet?"). Without that, a lost "I have room again" would leave both sides waiting for each other for ever.

### What arrives ahead of a gap

The receiver keeps up to four separate pieces that arrived after a gap, written straight into the receive buffer where they will belong, with a small list of which numbers it holds. When the missing segment comes, everything it joins up with is delivered at once:

```
expected: 1001
held:     [2461..3921) [3921..5381)  -> joined: [2461..5381)
arrives:  1001..2461   -> delivered 1001..5381, acknowledgment 5381
```

### The stack: which connection

`stack.rs` holds up to sixteen connections and eight listened-on ports. For each segment that arrives it finds the connection by all four numbers - both addresses' ports and the other machine's address - and hands it over. A SYN for a listened-on port makes a new connection if fewer than four are already waiting there for their program. Everything else is answered with a RST, made so that its sender believes it: with the sequence number the sender expects next. A RST itself is never answered - two machines could say "no" to each other for ever.

A connection's first number comes from a mixing function over the four numbers, a secret, and the clock (RFC 6528's idea), so that it cannot be guessed from outside. Anyone who can guess it can cut a connection off with a made-up RST. The function is a small integer mixer, not a cryptographic hash, and the secret is made from the clock, the card's address and the process number: guessable by anyone on the same wire. There is no source of randomness on this machine yet.

## The network service: a connection is a channel

To its program a connection is a channel, and what travels on it in each direction is one of three messages (`abi::net`):

```
[STREAM_DATA, bytes...]    the next bytes (up to 1023)
[STREAM_END]               no more will come this way
[STREAM_FAILED, status]    to the program only, and the last: it broke, and why
```

`TCP_CONNECT` (an address and a port, with a channel) is answered when the connection is made or known not to be. `TCP_LISTEN` (a port, with a channel) gives a *listener*. The program sends `[LISTENER_ACCEPT]` on it for each connection it is ready to take, and each connection arrives as a message carrying the other end of a new channel. A connection nobody has asked for stays with the service, in the backlog, so the other machine waits in the handshake instead of sending bytes nobody will read.

### Nobody drowns anybody

Two things are slower than each other in turn: the network, and the program. Neither may make the service keep unlimited bytes:

- **A program writing faster than the network.** The service takes a message from a connection's channel only when the connection has room for a whole one. Until then the service does not even wait on that channel; its messages pile up there, the channel fills (32 messages), and the program's `send` says `QueueFull`. The library waits and tries again. The program is held to the network's speed.
- **The network delivering faster than the program reads.** The service sends what arrived on to the program's channel until the channel is full. Then it stops taking bytes out of the connection, the connection's buffer fills, the window it offers shrinks to nothing, and the *other machine* stops sending. The program's slowness travels all the way back across the network.

A mutant of the first rule - watching the channel always - made the 1 MB echo test come back corrupted, because a message taken without room is cut short. That is the rule doing its work.

### Limits

One wait takes at most 32 handles. The card, the requests, sixteen connections, and now four listeners leave room for ten UDP sockets (there were sixteen):

```rust
const _: () = assert!(2 + SOCKETS + LISTENERS + TCP_CONNECTIONS <= abi::WAIT_HANDLES_MAX as usize);
```

A `const _: () = assert!(...)` is checked by the compiler: change one of the numbers so that they no longer fit, and the program does not build.

## Programs: `tcpsend`, `tcpecho`, and `TcpStream`

`taaj::net` gains `Net::tcp_connect` and `Net::tcp_listen`, `TcpStream` (`read`, `read_until`, `write`, `shutdown`) and `TcpListener` (`accept_until`). Two programs on the disk use them:

- `tcpsend ADDRESS PORT TEXT...` sends a line (or `fill N` bytes of a pattern), says it is done, and reads to the end. It prints the length and a Fletcher sum of what it sent and what came back, so a large transfer can be compared without printing it.
- `tcpecho [PORT [COUNT]]` sends back whatever each connection sends it.

`netinfo` has a new line: segments in, sent again, and answered "no such connection".

## C: the socket calls

A C program now gets `sys/socket.h`, `netinet/in.h` and `arpa/inet.h`. A socket is a descriptor in the same table as the files, so `read`, `write` and `close` work on it:

```c
int s = socket(AF_INET, SOCK_STREAM, 0);
struct sockaddr_in to = { .sin_family = AF_INET, .sin_port = htons(port) };
inet_pton(AF_INET, "10.0.2.2", &to.sin_addr);
connect(s, (struct sockaddr *)&to, sizeof to);
write(s, "hello\n", 6);
shutdown(s, SHUT_WR);
while ((n = read(s, buffer, sizeof buffer)) > 0) ...
```

Each call is a few lines in front of the Rust client (`libc/src/sockets.rs`): `connect` calls `tcp_connect`, `listen` calls `tcp_listen`, `accept` asks the listener and puts the stream in a new descriptor. The network service's reasons become Linux's errno numbers: refused is `ECONNREFUSED`, cut off is `ECONNRESET`, nobody there is `EHOSTUNREACH`, and a port in use is `EADDRINUSE`.

`htons` and friends turn a number round into network order: the most significant byte first, where the CPUs here keep it last. They and the address functions (`inet_pton`, `inet_ntop`, `inet_aton`, `inet_addr`, `inet_ntoa`) are pure arithmetic, in `libc/c/inet.c`, and are compared with the host's own on 266312 inputs. That comparison found that glibc refuses `010.0.2.2`: older readers took a 0 in front for octal (eight), newer ones for ten, so text two programs would read differently is no address. Mine refuses it now too.

`cnet` is the C program: `cnet send ADDRESS PORT WORD...` and `cnet serve PORT [COUNT]`, which answers each connection with how many bytes it got and what they were in upper case. It compiles unchanged on Linux with `cc cnet.c`, and I ran it there against itself before running it here.

## Testing against a TCP that is not mine

QEMU's user network ends every TCP connection inside the emulator and makes a new one from the host's own sockets (it is slirp, written long ago by other people). So a byte that gets from my machine to a Python program on the host was understood by a TCP that is not mine, in both directions. `kernel64/scripts/tcp-test.sh` boots once per CPU with two host ports passed on to the machine's ports 7 and 8000, and `tcp-host.py` plays the host:

| What | Checked by |
|---|---|
| `tcpsend` to the host: a line, then 300000 bytes and back | the sums match; the host counts 3 connections and 300029 bytes |
| `cnet send` from C to the host | the line comes back, then the end |
| a port nobody listens on, an address nobody has | `nothing is listening on that port there`, `nobody on this network has that address` |
| the host to `tcpecho`: 1 MB while reading at the same time | the same megabyte comes back |
| three connections at once to `tcpecho`, which serves one at a time | all three come back whole: two waited in the backlog |
| `cnet serve 7` while `tcpecho` has port 7 | `another program has that port` |
| one connection that sends nothing | it just ends: `0 bytes echoed` |
| the host to `cnet serve 8000`, twice | `you sent 11 bytes: HELLO CNET` and a longer one |
| `netinfo` | segments counted, none answered "no such connection" |

## Design decisions

| Decision | Chosen | Rejected, and why |
|---|---|---|
| Where TCP lives | One `Connection` type in the `net` crate, handed segments and the time, with `poll` for output | Inside the service: untestable without a machine. The host tests found the worst bug of this entry (see Bugs) in a simulation no boot would have reproduced |
| A connection to its program | A channel with three kinds of message | Shared memory rings: faster, but every program would need its own memory set up with the service, and a channel already gives waiting and "the other side has gone" |
| Back-pressure | The service stops watching a connection's channel while the connection has no room; stops reading out of the connection while the program's channel is full | Buffering in the service: unlimited memory for a program that never reads |
| Out-of-order segments | Kept, up to four pieces | Dropped (my first version): one loss then cost everything after it, and with backoff a lossy simulation took longer than an hour |
| Congestion window | Slow start and halving, roughly Reno | None: worked on a perfect network; on the simulated lossy one it turned every loss into a burst of losses |
| Accepting | The program says "one more" on the listener; connections wait in the service until then | Handing every connection over at once: a program that is busy would be given connections it cannot serve, and the other machines would send to them |
| First numbers | A mixing function of the ends, a secret and the clock | A counter: any machine on the network could guess the next one and cut connections off |
| TimeWait | Ten seconds | Two minutes, as RFC 9293 asks: matters on the Internet; here it would only hold ports and connection places |
| C sockets | Rust, in front of `taaj::net`, sharing the descriptor table | A second client of the protocol written in C: everything twice |
| Window | At most 65535 bytes (no window scaling) | Window scaling option: needs more buffer than 16 KiB per connection to matter |

## Bugs

- **A lossy simulation that never finished.** In the host test with ten segments in a hundred lost, one seed's connection gave up after five minutes of simulated time with every byte delivered except the last few. The trace showed the same segments being sent again every 60 seconds. Three things together: segments after a gap were dropped, so one loss meant sending everything after it again; the doubled wait was stored as the wait itself, so an acknowledgment did not bring it back down; and with no congestion window, each resend was a burst that the simulated network lost part of again. The fix was the three things described above: keep what arrives ahead of a gap, keep the doubling separate (`backoff`, reset by any acknowledgment), and hold the sender back.
- **My hand-made reference segment was wrong.** Its options were ten bytes, and a TCP header is counted in four-byte words: two bytes of padding were missing. My own parser read it correctly anyway, which is why the test said "read 2 payload bytes" instead of failing on the length.
- **65 segments sent twice on a network that loses nothing.** `netinfo` said 59 frames "not sent": TCP sends bursts faster than the card's driver drains its eight buffers, and the service dropped every frame the driver had no room for. TCP recovered, but each drop halved its speed and cost a timeout. Now a frame waits up to two ticks for the driver to have room, and if one waits in vain the card counts as stuck and frames are dropped at once until one gets through - so a hung driver does not hold the service up frame after frame. After the change: one segment sent twice in the same test.
- **Carriage returns.** The boot log's lines end in `\r\n`; my first test patterns ended in `$` and never matched.
- **An old test said TCP is a protocol nothing speaks.** It sent protocol 6 and expected "unsupported". It sends protocol 47 (GRE) now.
- **A SYN sent again was not counted** as sent again. Found by a stack test that compared the counter with the number of SYN+ACKs on the wire.
- Mutation testing found seven gaps in my tests, each with a test now (see below).

## How I checked it

Automated, on the host:

- `net`: 72 tests (30 before). The TCP ones include the handshake, both sides opening and both closing at once, numbers passing 0 and passing halfway round, a lost SYN, SYN+ACK, data, FIN and last ACK, three duplicate acknowledgments, every order of five segments, overlapping pieces, a FIN ahead of a gap, shut windows (for an hour of simulated time, asking with one byte), RSTs believed only with the exact number, and two connections over a simulated network losing 0, 5, 10 and 25 segments in a hundred, doubling and delaying others, for 60000 and 45000 bytes in the two directions over ten seeds. Two stacks wired together carry 70000 and 30000 bytes with nothing sent twice.
- `abi`: 22 tests; the connect request and the stream constants.
- `libc`: the errno table (4 tests, now with the socket errors), and `inet.c` against glibc: 266312 checks.

Automated, by booting:

- `make tcp64-test` and `make aarch64-tcp-test`: both pass. Both earlier network tests still pass with the changed service (`ip64-test` run after the change; the rest in `make test64`).

Mutation experiments: 42 mutants of `tcp.rs`, `stack.rs` and `wire.rs` on the host. Seven survived at first: the sequence comparison done with plain signed numbers (wrong only across halfway round, which no test crossed), the round-trip sample overwritten by every new segment, a window believed from an older segment, a stale timer kept when a shut window opens, the window not halved after a fast retransmit, a SYN+ACK opening a connection on a listened port, and the connection lookup ignoring the other machine's address. Each has a test now, and each of the seven is caught. Two mutants of the service, by booting: taking a program's message without room (caught: the megabyte came back corrupted) and never telling a program the other side is done (caught).

By hand: every step of the testing note below, with scripted boots (the keys typed through QEMU's monitor, `nc` on the host).

Not tested: a real network or a real card; a network that loses packets, except in the host simulation (QEMU's network loses nothing); a program that stops reading for a long time while the other side sends (the host tests do it for the connection; the service's half is only exercised by the megabyte echo); the network service being replaced while connections are open (they are lost, and their programs get `PeerClosed`); sixteen connections at once by booting.

## What is still missing

- UDP from C (`SOCK_DGRAM`), and sockets that do not wait (`O_NONBLOCK`, `select`, `poll`): a C program can serve one connection at a time.
- `connect` from a port chosen with `bind`.
- Window scaling, selective acknowledgments, timestamps, keep-alives, urgent data; waiting to send small pieces together (Nagle) or to acknowledge two segments in one.
- First numbers that cannot be guessed: that needs a source of randomness.
- Names (DNS), an address from the network (DHCP), IPv6.
- Step C: an HTTP server, serving a page that `curl` on the host fetches.

## What I learned

- TCP's whole design is one idea - number the bytes - and everything else (resending, dropping doubles, ordering, windows, closing) is a consequence of it.
- "Before" on numbers that go round is a subtraction read as signed, and the place where that matters is halfway round, not at 0. My tests crossed 0 and missed it; a mutant found it.
- Every protocol has to be written so that an *old* segment arriving late cannot undo anything: an old acknowledgment, an old window, an old FIN. Most of the conditions in `segment_synchronized` are exactly that.
- Dropping out-of-order segments and doubling the timeout are each fine alone. Together, on a lossy network, they make a connection that never finishes. A simulation that loses packets found it in a second; a boot would never have.
- Back-pressure through a chain of queues works by *not reading*: the service stops waiting on a full program's channel, and that silence travels back as a zero window to a machine on the other side of the network.
- The socket calls are a thin layer: once the Rust client existed, C's `connect` was twenty lines.

## Testing note

**1. Connections out of the machine (PC)**

On the host, in one terminal, something to connect to - a listener that prints what it gets:

```
nc -l 127.0.0.1 5009
```

In another terminal:

```
make run64
```

| Type (in QEMU) | You should see |
|---|---|
| `tcpsend 10.0.2.2 5009 hello from taaj` | `tcpsend: connected to 10.0.2.2:5009 from port 49152`, `tcpsend: 16 bytes sent, sum 335d05b2`; on the host, `nc` prints `hello from taaj` and ends - this `nc` ends as soon as the other side says it has no more to send - so the machine reads to the end at once: `tcpsend: 0 bytes back, sum 00000000, in … ms` |
| `tcpsend 10.0.2.2 5009` | `tcpsend: 10.0.2.2:5009: nothing is listening on that port there` (the `nc` has ended), and `tcpsend exited with code 1` |
| `tcpsend 10.0.2.99 80` | after three seconds: `tcpsend: 10.0.2.99:80: nobody on this network has that address` |
| `netinfo` | a last line `tcp      … segments in, … sent again, 0 answered "no such connection"` |

**2. Connections into the machine, and C**

Start the machine with two of the host's ports passed on to the machine's ports 7 and 8000:

```
make run64 RUN_NET="-netdev user,id=n0,hostfwd=tcp:127.0.0.1:5007-:7,hostfwd=tcp:127.0.0.1:5080-:8000 -device virtio-net-pci,netdev=n0"
```

| Type (in QEMU) | You should see |
|---|---|
| `tcpecho &` | `[1] tcpecho`, and `tcpecho: listening on port 7` |

On the host:

```
echo hello there | nc -N 127.0.0.1 5007
```

| Look for | Meaning |
|---|---|
| `hello there` printed by `nc` | the line went in and came back |
| in QEMU: `tcpecho: 12 bytes echoed to 10.0.2.2:…` | the machine served the connection to its end |

| Type (in QEMU) | You should see |
|---|---|
| `cnet serve 7 1` | `cnet: port 7: another program has that port` |
| `cnet serve 8000 1` | `cnet: listening on port 8000`, and it waits |

On the host:

```
echo hi from the host | nc -N 127.0.0.1 5080
```

| Look for | Meaning |
|---|---|
| `you sent 17 bytes: HI FROM THE HOST` printed by `nc` | a C program accepted the connection, read to the end and answered |
| in QEMU: `cnet: 17 bytes from 10.0.2.2:…, answered` | |

And C going out. On the host: `nc -l 127.0.0.1 5009`, then:

| Type (in QEMU) | You should see |
|---|---|
| `cnet send 10.0.2.2 5009 hello from c` | `cnet: connected to 10.0.2.2:5009 from port …`; `nc` prints `hello from c` and ends; then `cnet: 0 bytes back, and the other side is done` |

**3. The same on AArch64**

On the host: `nc -l 127.0.0.1 5009`. Then:

```
make run-aarch64
```

The terminal is the keyboard and screen. To leave QEMU: **Ctrl+A, then X**.

| Type | You should see |
|---|---|
| `tcpsend 10.0.2.2 5009 hello from arm` | `tcpsend: connected to 10.0.2.2:5009 from port 49152`, `tcpsend: 15 bytes sent, sum …`; `nc` prints `hello from arm` and ends; `tcpsend: 0 bytes back, sum 00000000, in … ms` |
| `tcpsend 10.0.2.2 5009` | `tcpsend: 10.0.2.2:5009: nothing is listening on that port there` |

**4. The tests by themselves**

```
make tcp64-test
make aarch64-tcp-test
```

Each prints the host's verdict and ends `tcp64-test passed` / `aarch64-tcp-test passed`:

| Look for | Meaning |
|---|---|
| `host: ok: 5 checks; 3 connections from the machine served, 300029 bytes` | every connection into the machine came back whole, and the three out of it arrived |

Host checks:

```
cd net && cargo test           # 72 tests: the protocols, TCP and the stack
cd kernel/abi && cargo test    # 22 tests
make libc-test                 # errno, and inet.c against this machine's own
```

Automated: `make test64`
