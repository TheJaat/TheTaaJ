//! net - the network service: this machine on its network.
//!
//! It owns the card's frames of two kinds - IPv4 and ARP - and is the
//! only program that sees them. Everything a machine on a network must
//! do by itself happens here: answering "who has this address?",
//! answering pings, finding the card a packet must be handed to. And
//! it is where programs get the network from (`abi::net`): a ping, a
//! UDP port to send and receive datagrams on, or a TCP connection -
//! made from here, or accepted on a port that is listened on.
//!
//! Startup handles, from init:
//!   1  the request channel (abi::net)
//!   2  the channel to the registry, on which it asks for the one
//!      service it uses: "nic", the card's driver
//!
//! All of the protocols are in the `net` crate, which knows nothing of
//! cards, clocks or channels and is tested on the host. This program
//! is the plumbing around it: frames from the driver in, frames to the
//! driver out, the tick count as the time, and a table of who has
//! which port.
//!
//! A TCP connection is, to its program, a channel (`abi::net`,
//! "streams"): bytes to send arrive on it as messages, bytes that
//! came are sent on it as messages. Neither side may drown the other:
//!
//!   - this service takes a message from a program only when the
//!     connection has room for all of it. Until then the messages wait
//!     in the channel, the channel fills up, and the program's `send`
//!     says "full": it has to wait.
//!   - and when a program does not read, its channel fills up, this
//!     service stops taking bytes out of the connection, the
//!     connection's own buffer fills, and the window it offers the
//!     other machine shrinks to nothing: the sender across the network
//!     has to wait. The program's slowness travels all the way back.
//!
//! The address is fixed, for now: 10.0.2.15 in the network 10.0.2.0/24
//! with its router at 10.0.2.2 - what QEMU's own little network gives a
//! machine. Asking the network for an address (DHCP) is not written.
//!
//! When the card's driver is replaced, the frames stop coming (the
//! channel they came on closes): this service asks the new driver for
//! them again. When this service is replaced, its sockets are gone:
//! the programs that had them are told by their channels closing.

#![no_std]
#![no_main]

use abi::net::{self as protocol, Info, Ping};
use net::stack::{Config, Received, Sent, Stack, Wire, TCP_CONNECTIONS};
use net::tcp::{Failure, State};
use net::wire::ethernet;
use net::Ipv4;
use taaj::nic::{Frames, Nic};
use taaj::services::Services;
use taaj::{println, Error, Handle};

taaj::program!(main);

const ADDRESS: Ipv4 = Ipv4([10, 0, 2, 15]);
const PREFIX: u8 = 24;
const GATEWAY: Ipv4 = Ipv4([10, 0, 2, 2]);

/// The frame types this service takes from the card.
const TYPES: [u16; 2] = [ethernet::TYPE_IPV4, ethernet::TYPE_ARP];

/// How many UDP ports, and how many TCP ports listened on, there can
/// be at once. (With the connections, the card and the requests they
/// are everything this service waits on, and one wait takes at most
/// `abi::WAIT_HANDLES_MAX` things.)
const SOCKETS: usize = 10;
const LISTENERS: usize = 4;
const _: () = assert!(2 + SOCKETS + LISTENERS + TCP_CONNECTIONS <= abi::WAIT_HANDLES_MAX as usize);
const PINGS: usize = 8;
/// Where ports given out to programs that asked for "any" begin (the
/// range set aside for that: 49152 to 65535).
const FIRST_FREE_PORT: u16 = 49152;
/// The stack counts in milliseconds; a tick is ten.
const MS_PER_TICK: u64 = 10;
/// How many frames are taken from the driver before anyone else gets a
/// turn: a flood from the wire must not starve the programs here.
const FRAMES_AT_ONCE: usize = 16;

/// A port, and the channel that is its socket.
#[derive(Clone, Copy)]
struct Socket {
    port: u16,
    channel: Handle,
}

/// A TCP port that is listened on.
#[derive(Clone, Copy)]
struct Listener {
    port: u16,
    channel: Handle,
    /// How many connections its program has said it will take.
    wanted: u32,
}

/// A TCP connection's other half: the channel to its program. (Which
/// connection: the one with the same number in the stack.)
#[derive(Clone, Copy)]
struct Stream {
    channel: Handle,
    /// Where to say that the connection is made, or why it is not -
    /// for one made from here, until that is known.
    connecting: Option<Handle>,
    /// The program has said it has no more to send.
    shut: bool,
    /// The program has been told the other side has no more to send.
    end_told: bool,
}

/// What is being waited on, besides the card and the requests.
#[derive(Clone, Copy)]
enum Watched {
    Socket(usize),
    Listener(usize),
    Stream(usize),
}

/// A ping on its way: who to tell, and what its answer must look like.
#[derive(Clone, Copy)]
struct Waiting {
    reply: Handle,
    to: Ipv4,
    /// The machine on this wire it was handed to: if nobody has *that*
    /// address, the ping went nowhere.
    hop: Option<Ipv4>,
    identifier: u16,
    sequence: u16,
    length: usize,
    deadline: u64,
}

/// The data a ping carries: any bytes would do; these make a packet
/// that came back wrong visible.
fn ping_data(identifier: u16, n: usize) -> u8 {
    (n as u8).wrapping_mul(7).wrapping_add(identifier as u8)
}

/// Frames out: to the card's driver.
///
/// When the driver has no room (all of its buffers are with the card),
/// the frame waits a little for one to come free, as an operating
/// system holds a frame back when its card's queue is full. TCP sends
/// in bursts, and a frame dropped here would cost a connection a
/// timeout and half its speed, for a loss the network never had.
/// But a driver that does not drain at all must not hold this service
/// up frame after frame: after one wait in vain the card counts as
/// stuck, and frames are dropped (and counted) at once until one gets
/// through.
struct Card<'a> {
    card: &'a mut Nic,
    lost: &'a mut u64,
    stuck: &'a mut bool,
}

/// How long a frame waits for the driver to have room, in ticks.
const CARD_WAIT: u64 = 2;

impl Wire for Card<'_> {
    fn send(&mut self, frame: &[u8]) {
        let deadline = taaj::ticks() + CARD_WAIT;
        loop {
            match self.card.send(frame) {
                Ok(()) => {
                    *self.stuck = false;
                    return;
                }
                Err(Error::QueueFull) if !*self.stuck && taaj::ticks() < deadline => taaj::yield_now(),
                Err(Error::QueueFull) => {
                    *self.stuck = true;
                    break;
                }
                Err(_) => break,
            }
        }
        *self.lost += 1;
    }
}

struct Service {
    stack: Stack,
    sockets: [Option<Socket>; SOCKETS],
    listeners: [Option<Listener>; LISTENERS],
    streams: [Option<Stream>; TCP_CONNECTIONS],
    /// A program's channel was full: try again at this tick.
    retry: Option<u64>,
    pings: [Option<Waiting>; PINGS],
    /// The next ping's identifier: a new one each, so that an answer
    /// that comes late is not taken for the answer to another.
    identifier: u16,
    /// Where to look first for a free port: UDP's, and TCP's (the two
    /// have nothing to do with each other).
    next_port: u16,
    next_tcp_port: u16,
    no_such_port: u64,
    not_taken: u64,
    /// Frames the driver had no room for.
    lost: u64,
    /// The driver has not taken a frame however long it was waited for.
    stuck: bool,
}

fn answer(reply: Handle, message: &[u8]) {
    let _ = taaj::send(reply, message, &[]);
    let _ = taaj::close(reply);
}

impl Service {
    fn info(&self) -> Info {
        let config = self.stack.config();
        let counted = self.stack.counters();
        let mut counters = [0u64; protocol::COUNTERS];
        counters[protocol::FRAMES_IN] = counted.frames_in;
        counters[protocol::FRAMES_OUT] = counted.frames_out;
        counters[protocol::ARP_ANSWERED] = counted.arp_answered;
        counters[protocol::ECHOES_ANSWERED] = counted.echoes_answered;
        counters[protocol::DATAGRAMS_IN] = counted.datagrams_in;
        counters[protocol::BAD] = counted.bad;
        counters[protocol::NOT_FOR_US] = counted.not_for_us;
        counters[protocol::UNSUPPORTED] = counted.unsupported;
        counters[protocol::UNDELIVERED] = counted.undelivered + self.lost;
        counters[protocol::NO_SUCH_PORT] = self.no_such_port;
        counters[protocol::NOT_TAKEN] = self.not_taken;
        counters[protocol::SEGMENTS_IN] = counted.segments_in;
        counters[protocol::SEGMENTS_AGAIN] = counted.segments_again;
        counters[protocol::SEGMENTS_REFUSED] = counted.refused;
        Info { mac: config.mac, address: config.address.0, prefix: config.prefix, gateway: config.gateway.0, counters }
    }

    fn close_socket(&mut self, n: usize) {
        if let Some(socket) = self.sockets[n].take() {
            let _ = taaj::close(socket.channel);
        }
    }

    /// One frame from the card, through the stack; what comes out for
    /// a program is passed on.
    fn frame(&mut self, frame: &[u8], card: &mut Nic, now: u64) {
        let mut wire = Card { card, lost: &mut self.lost, stuck: &mut self.stuck };
        match self.stack.receive(frame, now * MS_PER_TICK, &mut wire) {
            Received::Udp { from, from_port, to_port, payload, packet } => {
                let Some(n) = self.sockets.iter().position(|socket| socket.is_some_and(|socket| socket.port == to_port)) else {
                    self.no_such_port += 1;
                    self.stack.refuse_udp(packet, now * MS_PER_TICK, &mut wire);
                    return;
                };
                let mut message = [0u8; abi::MESSAGE_MAX as usize];
                let Some(length) = protocol::datagram_encode(&mut message, from.0, from_port, payload) else {
                    self.not_taken += 1; // more than a message carries
                    return;
                };
                let channel = self.sockets[n].map_or(0, |socket| socket.channel);
                match taaj::send(channel, &message[..length], &[]) {
                    Ok(()) => {}
                    // The program is not reading: its loss, and nobody
                    // else's wait.
                    Err(Error::QueueFull) => self.not_taken += 1,
                    Err(_) => {
                        self.not_taken += 1;
                        self.close_socket(n);
                    }
                }
            }
            Received::EchoReply { from, identifier, sequence, ttl, data } => {
                let matches = |ping: &Waiting| {
                    ping.identifier == identifier
                        && ping.sequence == sequence
                        && ping.to == from
                        && data.len() == ping.length
                        && data.iter().enumerate().all(|(n, &byte)| byte == ping_data(identifier, n))
                };
                if let Some(slot) = self.pings.iter_mut().find(|slot| slot.as_ref().is_some_and(matches)) {
                    if let Some(ping) = slot.take() {
                        answer(ping.reply, &[protocol::OK, from.0[0], from.0[1], from.0[2], from.0[3], ttl]);
                    }
                }
            }
            Received::Handled | Received::Dropped(_) => {}
        }
    }

    /// The clock: questions asked again or given up, pings waited for
    /// long enough.
    fn tick(&mut self, card: &mut Nic, now: u64) {
        let mut gone = [None; 4];
        let mut count = 0;
        self.stack.tick(now * MS_PER_TICK, &mut Card { card, lost: &mut self.lost, stuck: &mut self.stuck }, &mut |hop| {
            if count < gone.len() {
                gone[count] = Some(hop);
                count += 1;
            }
        });
        for slot in &mut self.pings {
            let Some(ping) = *slot else { continue };
            // Nobody has the address it was to be handed to: given up
            // on, or - when the wait is over first - not heard of yet.
            let nobody = |hop: Ipv4| self.stack.neighbour(hop, now * MS_PER_TICK).is_none();
            let status = if ping.hop.is_some() && gone[..count].contains(&ping.hop) {
                protocol::UNREACHABLE
            } else if now >= ping.deadline {
                if ping.hop.is_some_and(nobody) { protocol::UNREACHABLE } else { protocol::TIMED_OUT }
            } else {
                continue;
            };
            answer(ping.reply, &[status]);
            *slot = None;
        }
    }

    /// When `tick` next has something to do, in ticks.
    fn next_timer(&self) -> u64 {
        let stack = self.stack.next_timer().map(|ms| ms.div_ceil(MS_PER_TICK));
        let pings = self.pings.iter().flatten().map(|ping| ping.deadline).min();
        stack.into_iter().chain(pings).chain(self.retry).min().unwrap_or(u64::MAX)
    }

    fn ping(&mut self, ping: Ping, reply: Handle, card: &mut Nic, now: u64) {
        let Some(slot) = self.pings.iter().position(|slot| slot.is_none()) else {
            return answer(reply, &[protocol::NO_ROOM]);
        };
        self.identifier = self.identifier.wrapping_add(1);
        let identifier = self.identifier;
        let to = Ipv4(ping.to);
        let mut data = [0u8; protocol::PING_DATA_MAX as usize];
        let length = usize::from(ping.length);
        for (n, byte) in data[..length].iter_mut().enumerate() {
            *byte = ping_data(identifier, n);
        }
        let mut wire = Card { card, lost: &mut self.lost, stuck: &mut self.stuck };
        match self.stack.send_echo(to, identifier, ping.sequence, &data[..length], now * MS_PER_TICK, &mut wire) {
            Sent::Sent | Sent::Waiting => {
                self.pings[slot] = Some(Waiting {
                    reply,
                    to,
                    hop: self.stack.next_hop(to),
                    identifier,
                    sequence: ping.sequence,
                    length,
                    deadline: now + u64::from(ping.wait),
                });
            }
            Sent::NoRoom => answer(reply, &[protocol::NO_ROOM]),
            Sent::TooBig => answer(reply, &[protocol::TOO_BIG]),
            Sent::NoRoute => answer(reply, &[protocol::NO_ROUTE]),
        }
    }

    /// A port for a connection made from here, or for a listener that
    /// asked for any: the next one nothing uses. (There are far more
    /// ports than connections: one is always free.)
    fn free_tcp_port(&mut self) -> u16 {
        loop {
            let port = self.next_tcp_port;
            self.next_tcp_port = port.checked_add(1).unwrap_or(FIRST_FREE_PORT);
            if self.stack.tcp_port_free(port) {
                return port;
            }
        }
    }

    fn connect(&mut self, to: Ipv4, to_port: u16, reply: Handle, channel: Handle, card: &mut Nic, now: u64) {
        let port = self.free_tcp_port();
        let mut wire = Card { card, lost: &mut self.lost, stuck: &mut self.stuck };
        match self.stack.tcp_connect(port, to, to_port, now * MS_PER_TICK, &mut wire) {
            // The answer comes when the connection is made, or is
            // known not to be (`pump`).
            Ok(n) => self.streams[n] = Some(Stream { channel, connecting: Some(reply), shut: false, end_told: false }),
            Err(why) => {
                let _ = taaj::close(channel);
                answer(reply, &[if why == Sent::NoRoute { protocol::NO_ROUTE } else { protocol::NO_ROOM }]);
            }
        }
    }

    fn listen(&mut self, asked: u16, reply: Handle, channel: Handle) {
        let refuse = |status: u8| {
            let _ = taaj::close(channel);
            answer(reply, &[status]);
        };
        let Some(slot) = self.listeners.iter().position(|listener| listener.is_none()) else {
            return refuse(protocol::NO_ROOM);
        };
        let port = if asked != 0 { asked } else { self.free_tcp_port() };
        if !self.stack.tcp_port_free(port) {
            return refuse(protocol::IN_USE);
        }
        if !self.stack.tcp_listen(port) {
            return refuse(protocol::NO_ROOM);
        }
        self.listeners[slot] = Some(Listener { port, channel, wanted: 0 });
        let [low, high] = port.to_le_bytes();
        answer(reply, &[protocol::OK, low, high]);
    }

    /// Listener `n` has something: its program will take one more
    /// connection, or has gone.
    fn listener(&mut self, n: usize, card: &mut Nic, now: u64) {
        let Some(listener) = self.listeners[n].as_mut() else { return };
        let mut message = [0u8; 8];
        let mut handles = [0u64; abi::MESSAGE_HANDLES_MAX as usize];
        match taaj::receive(listener.channel, &mut message, &mut handles) {
            Ok((length, count)) => {
                for &handle in &handles[..count] {
                    let _ = taaj::close(handle);
                }
                if message[..length] == [protocol::LISTENER_ACCEPT] {
                    listener.wanted = listener.wanted.saturating_add(1);
                }
            }
            Err(_) => {
                let _ = taaj::close(listener.channel);
                let port = listener.port;
                self.listeners[n] = None;
                self.stack.tcp_unlisten(port, now * MS_PER_TICK, &mut Card { card, lost: &mut self.lost, stuck: &mut self.stuck });
            }
        }
    }

    /// Stream `n`'s program has sent something - bytes to send, or "no
    /// more" - or has gone.
    fn stream(&mut self, n: usize, card: &mut Nic, now: u64) {
        let Some(stream) = self.streams[n].as_mut() else { return };
        let mut message = [0u8; abi::MESSAGE_MAX as usize];
        let mut handles = [0u64; abi::MESSAGE_HANDLES_MAX as usize];
        let mut wire = Card { card, lost: &mut self.lost, stuck: &mut self.stuck };
        match taaj::receive(stream.channel, &mut message, &mut handles) {
            Ok((length, count)) => {
                for &handle in &handles[..count] {
                    let _ = taaj::close(handle);
                }
                match &message[..length] {
                    // (It is only taken from the channel when the
                    // connection has room for a whole message.)
                    [protocol::STREAM_DATA, data @ ..] if !stream.shut => {
                        self.stack.tcp_write(n, data, now * MS_PER_TICK, &mut wire);
                    }
                    [protocol::STREAM_END] if !stream.shut => {
                        stream.shut = true;
                        self.stack.tcp_shutdown(n, now * MS_PER_TICK, &mut wire);
                    }
                    _ => {}
                }
            }
            // The program has gone: what it wrote is still sent, and
            // the connection closed in order - by the stack alone.
            Err(_) => {
                let _ = taaj::close(stream.channel);
                self.streams[n] = None;
                self.stack.tcp_close(n, now * MS_PER_TICK, &mut wire);
            }
        }
    }

    /// Passes on what the connections have for their programs: that a
    /// connection is made, the bytes that arrived, that the other side
    /// is done, that it is over. Called after anything that may have
    /// changed a connection - a frame, the clock, a program's message.
    fn pump(&mut self, card: &mut Nic, now: u64) {
        let ms = now * MS_PER_TICK;
        let mut wire = Card { card, lost: &mut self.lost, stuck: &mut self.stuck };
        self.retry = None;

        // New connections, to the listeners that will take them.
        for listener in self.listeners.iter_mut().flatten() {
            while listener.wanted > 0 {
                let Some((n, from, from_port)) = self.stack.tcp_accept(listener.port) else { break };
                listener.wanted -= 1;
                let mut message = [0u8; protocol::DATAGRAM_HEADER];
                let handed = taaj::channel_create().and_then(|(ours, theirs)| {
                    let _ = protocol::datagram_encode(&mut message, from.0, from_port, &[]);
                    taaj::send(listener.channel, &message, &[theirs]).map(|()| ours).inspect_err(|_| {
                        let _ = taaj::close(ours);
                        let _ = taaj::close(theirs);
                    })
                });
                match handed {
                    Ok(ours) => self.streams[n] = Some(Stream { channel: ours, connecting: None, shut: false, end_told: false }),
                    // Its program is not reading, or has gone (which
                    // `listener` will notice): nobody to give it to.
                    Err(_) => self.stack.tcp_abort(n, ms, &mut wire),
                }
            }
        }

        let mut message = [0u8; abi::MESSAGE_MAX as usize];
        for n in 0..TCP_CONNECTIONS {
            let Some(mut stream) = self.streams[n] else { continue };
            let Some((state, failure)) = self.stack.tcp(n).map(|connection| (connection.state(), connection.failure())) else {
                continue;
            };
            // Over: the program is told why (if it was not by both
            // sides saying so), its channel closed, and the stack may
            // forget the connection.
            let over = |stack: &mut Stack, wire: &mut Card<'_>, stream: &Stream, status: Option<u8>| {
                if let Some(status) = status {
                    let _ = taaj::send(stream.channel, &[protocol::STREAM_FAILED, status], &[]);
                }
                let _ = taaj::close(stream.channel);
                stack.tcp_close(n, ms, wire);
            };
            let status = failure.map(|failure| match failure {
                Failure::Refused => protocol::REFUSED,
                Failure::Reset => protocol::RESET,
                Failure::TimedOut => protocol::TIMED_OUT,
                Failure::Unreachable => protocol::UNREACHABLE,
            });
            if let Some(reply) = stream.connecting {
                match state {
                    State::SynSent | State::SynReceived => continue,
                    State::Closed => {
                        answer(reply, &[status.unwrap_or(protocol::RESET)]);
                        over(&mut self.stack, &mut wire, &stream, None);
                        self.streams[n] = None;
                        continue;
                    }
                    _ => {
                        let [low, high] = self.stack.tcp_local_port(n).unwrap_or(0).to_le_bytes();
                        answer(reply, &[protocol::OK, low, high]);
                        stream.connecting = None;
                    }
                }
            }
            // What arrived, a message at a time, for as long as the
            // program's channel takes it.
            let mut full = false;
            let mut gone = false;
            loop {
                let count = self.stack.tcp_peek(n, &mut message[1..]);
                if count == 0 {
                    break;
                }
                message[0] = protocol::STREAM_DATA;
                match taaj::send(stream.channel, &message[..1 + count], &[]) {
                    Ok(()) => self.stack.tcp_consume(n, count, ms, &mut wire),
                    Err(Error::QueueFull) => full = true,
                    Err(_) => gone = true,
                }
                if full || gone {
                    break;
                }
            }
            let finished = self.stack.tcp(n).is_some_and(|connection| connection.finished());
            if finished && !stream.end_told && !full && !gone {
                match taaj::send(stream.channel, &[protocol::STREAM_END], &[]) {
                    Ok(()) => stream.end_told = true,
                    Err(Error::QueueFull) => full = true,
                    Err(_) => gone = true,
                }
            }
            if gone {
                over(&mut self.stack, &mut wire, &stream, None);
                self.streams[n] = None;
            } else if full {
                // Nothing says when a channel has room again: look
                // again at the next tick.
                self.retry = Some(now + 1);
                self.streams[n] = Some(stream);
            } else if state == State::Closed {
                over(&mut self.stack, &mut wire, &stream, status);
                self.streams[n] = None;
            } else {
                self.streams[n] = Some(stream);
            }
        }
    }

    /// What to wait on besides the card and the requests: written to
    /// `handles` and `which`, the count returned. A stream's channel
    /// is among them only while its connection has room for a whole
    /// message (or expects none: then it is watched for its program
    /// going away) - that is what makes a program wait when the
    /// network is slower than it is.
    fn watched(&self, handles: &mut [Handle], which: &mut [Watched]) -> usize {
        let sockets = self.sockets.iter().enumerate().filter_map(|(n, socket)| Some((socket.as_ref()?.channel, Watched::Socket(n))));
        let listeners = self.listeners.iter().enumerate().filter_map(|(n, listener)| Some((listener.as_ref()?.channel, Watched::Listener(n))));
        let streams = self.streams.iter().enumerate().filter_map(|(n, stream)| {
            let stream = stream.as_ref()?;
            let room = self.stack.tcp(n).map_or(0, |connection| connection.writable());
            (stream.connecting.is_none() && (stream.shut || room >= protocol::STREAM_DATA_MAX)).then_some((stream.channel, Watched::Stream(n)))
        });
        let mut count = 0;
        for (handle, what) in sockets.chain(listeners).chain(streams) {
            handles[count] = handle;
            which[count] = what;
            count += 1;
        }
        count
    }

    fn bind(&mut self, asked: u16, reply: Handle, channel: Handle) {
        let taken = |sockets: &[Option<Socket>; SOCKETS], port: u16| sockets.iter().flatten().any(|socket| socket.port == port);
        let refuse = |status: u8| {
            let _ = taaj::close(channel);
            answer(reply, &[status]);
        };
        let Some(slot) = self.sockets.iter().position(|socket| socket.is_none()) else {
            return refuse(protocol::NO_ROOM);
        };
        let port = if asked != 0 {
            if taken(&self.sockets, asked) {
                return refuse(protocol::IN_USE);
            }
            asked
        } else {
            // Any free one: the next not in use, going round. (There
            // are far more ports than sockets: one is always free.)
            while taken(&self.sockets, self.next_port) {
                self.next_port = self.next_port.checked_add(1).unwrap_or(FIRST_FREE_PORT);
            }
            let port = self.next_port;
            self.next_port = port.checked_add(1).unwrap_or(FIRST_FREE_PORT);
            port
        };
        self.sockets[slot] = Some(Socket { port, channel });
        let [low, high] = port.to_le_bytes();
        answer(reply, &[protocol::OK, low, high]);
    }

    /// One message on the request channel.
    fn request(&mut self, request: &[u8], handles: &[Handle], card: &mut Nic, now: u64) {
        match (request, handles) {
            ([protocol::INFO], &[reply]) => {
                let mut message = [0u8; 1 + Info::SIZE];
                message[0] = protocol::OK;
                message[1..].copy_from_slice(&self.info().to_bytes());
                answer(reply, &message);
            }
            ([protocol::PING, ..], &[reply]) => match Ping::from_bytes(request) {
                Some(ping) => self.ping(ping, reply, card, now),
                None => answer(reply, &[protocol::BAD_REQUEST]),
            },
            (&[protocol::UDP_BIND, low, high], &[reply, channel]) => self.bind(u16::from_le_bytes([low, high]), reply, channel),
            (&[protocol::TCP_LISTEN, low, high], &[reply, channel]) => self.listen(u16::from_le_bytes([low, high]), reply, channel),
            ([protocol::TCP_CONNECT, ..], &[reply, channel]) => match protocol::connect_decode(request) {
                Some((to, port)) => self.connect(Ipv4(to), port, reply, channel, card, now),
                None => {
                    let _ = taaj::close(channel);
                    answer(reply, &[protocol::BAD_REQUEST]);
                }
            },
            // Not a request: answered if there is somewhere to answer,
            // and whatever else came with it is not kept.
            (_, handles) => {
                for &handle in handles.iter().skip(1) {
                    let _ = taaj::close(handle);
                }
                if let Some(&reply) = handles.first() {
                    answer(reply, &[protocol::BAD_REQUEST]);
                }
            }
        }
    }

    /// Socket `n` has something: a datagram to send, or its program
    /// has gone.
    fn socket(&mut self, n: usize, card: &mut Nic, now: u64) {
        let Some(socket) = self.sockets[n] else { return };
        let mut message = [0u8; abi::MESSAGE_MAX as usize];
        let mut handles = [0u64; abi::MESSAGE_HANDLES_MAX as usize];
        match taaj::receive(socket.channel, &mut message, &mut handles) {
            Ok((length, count)) => {
                // (Handles are not part of a datagram.)
                for &handle in &handles[..count] {
                    let _ = taaj::close(handle);
                }
                if let Some((to, port, data)) = protocol::datagram_decode(&message[..length]) {
                    let mut wire = Card { card, lost: &mut self.lost, stuck: &mut self.stuck };
                    // What became of it is counted by the stack; the
                    // program is not told, as on any network.
                    self.stack.send_udp(socket.port, Ipv4(to), port, data, now * MS_PER_TICK, &mut wire);
                }
            }
            Err(_) => self.close_socket(n),
        }
    }
}

/// Asks the card's driver for this service's frames, for as long as it
/// takes: a driver being replaced answers when the new one runs.
fn listen(card: &Nic) -> Frames {
    loop {
        if let Ok(frames) = card.listen(&TYPES) {
            return frames;
        }
        taaj::sleep(10);
    }
}

fn main() -> i64 {
    let (Some(requests), Some(registry)) = (taaj::startup_handle(0), taaj::startup_handle(1)) else {
        println!("[net] started without its channels");
        return 1;
    };
    let Ok(driver) = Services::new(registry).lookup("nic") else {
        println!("[net] no network card's driver to be found");
        return 2;
    };
    let mut card = Nic::new(driver);
    let described = match card.info() {
        Ok(info) => info,
        Err(error) => {
            println!("[net] {}", error.describe());
            return 3;
        }
    };
    let mut frames = listen(&card);
    let mac = described.address;
    println!(
        "[net] this machine is {ADDRESS}/{PREFIX} on card {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}, with its router at {GATEWAY}{}",
        mac[0],
        mac[1],
        mac[2],
        mac[3],
        mac[4],
        mac[5],
        if described.link_up { "" } else { " - the link is down" }
    );

    let mut service = Service {
        stack: Stack::new(Config { mac, address: ADDRESS, prefix: PREFIX, gateway: GATEWAY }),
        sockets: [None; SOCKETS],
        listeners: [None; LISTENERS],
        streams: [None; TCP_CONNECTIONS],
        retry: None,
        next_tcp_port: FIRST_FREE_PORT,
        pings: [None; PINGS],
        identifier: taaj::process_id() as u16,
        next_port: FIRST_FREE_PORT,
        no_such_port: 0,
        not_taken: 0,
        lost: 0,
        stuck: false,
    };

    // Where the connections' numbering starts should be a secret. This
    // machine has nothing random to make one from: the clock, the
    // card's address and this process's number are the best there is,
    // and anyone on the same wire can guess all three.
    service.stack.seed((taaj::ticks() as u32).wrapping_mul(0x9E37_79B9) ^ u32::from_le_bytes([mac[2], mac[3], mac[4], mac[5]]) ^ (taaj::process_id() as u32) << 20);

    let mut frame = [0u8; abi::nic::FRAME_MAX];
    let mut message = [0u8; abi::MESSAGE_MAX as usize];
    loop {
        let now = taaj::ticks();
        service.tick(&mut card, now);
        service.pump(&mut card, now);

        // The card, a request, a socket, a listener or a stream - or
        // the clock.
        let mut watched = [0u64; abi::WAIT_HANDLES_MAX as usize];
        let mut which = [Watched::Socket(0); abi::WAIT_HANDLES_MAX as usize];
        watched[0] = frames.channel();
        watched[1] = requests;
        let count = 2 + service.watched(&mut watched[2..], &mut which[2..]);
        let ready = match taaj::object_wait_until(&watched[..count], service.next_timer()) {
            Ok(Some(ready)) => ready,
            Ok(None) => continue,
            Err(error) => {
                println!("[net] cannot wait: {error:?}");
                return 4;
            }
        };
        let now = taaj::ticks();
        match ready {
            0 => {
                for _ in 0..FRAMES_AT_ONCE {
                    if !matches!(taaj::poll(&[frames.channel()]), Ok(Some(_))) {
                        break;
                    }
                    // (A long frame is two messages: the second is on
                    // its way if it is not there yet.)
                    match frames.next_until(&mut frame, now + 2) {
                        Ok(Some(length)) => service.frame(&frame[..length], &mut card, now),
                        Ok(None) => {}
                        // The driver has gone, and init is starting
                        // another: ask that one.
                        Err(_) => {
                            frames = listen(&card);
                            println!("[net] the card's driver was replaced: listening to the new one");
                            break;
                        }
                    }
                }
            }
            1 => {
                let mut handles = [0u64; abi::MESSAGE_HANDLES_MAX as usize];
                match taaj::receive(requests, &mut message, &mut handles) {
                    Ok((length, handed)) => service.request(&message[..length], &handles[..handed], &mut card, now),
                    Err(Error::PeerClosed) => return 0, // no clients left, and none can come
                    Err(error) => {
                        println!("[net] cannot receive: {error:?}");
                        return 4;
                    }
                }
            }
            n => match which[n] {
                Watched::Socket(n) => service.socket(n, &mut card, now),
                Watched::Listener(n) => service.listener(n, &mut card, now),
                Watched::Stream(n) => service.stream(n, &mut card, now),
            },
        }
    }
}
