//! net - the network service: this machine on its network.
//!
//! It owns the card's frames of two kinds - IPv4 and ARP - and is the
//! only program that sees them. Everything a machine on a network must
//! do by itself happens here: answering "who has this address?",
//! answering pings, finding the card a packet must be handed to. And
//! it is where programs get the network from (`abi::net`): a ping, or
//! a UDP port to send and receive datagrams on.
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
use net::stack::{Config, Received, Sent, Stack, Wire};
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

const SOCKETS: usize = 16;
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

/// Frames out: to the card's driver. One it has no room for is lost,
/// like any frame a wire loses - and counted.
struct Card<'a> {
    card: &'a mut Nic,
    lost: &'a mut u64,
}

impl Wire for Card<'_> {
    fn send(&mut self, frame: &[u8]) {
        if self.card.send(frame).is_err() {
            *self.lost += 1;
        }
    }
}

struct Service {
    stack: Stack,
    sockets: [Option<Socket>; SOCKETS],
    pings: [Option<Waiting>; PINGS],
    /// The next ping's identifier: a new one each, so that an answer
    /// that comes late is not taken for the answer to another.
    identifier: u16,
    /// Where to look first for a free port.
    next_port: u16,
    no_such_port: u64,
    not_taken: u64,
    /// Frames the driver had no room for.
    lost: u64,
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
        let mut wire = Card { card, lost: &mut self.lost };
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
        self.stack.tick(now * MS_PER_TICK, &mut Card { card, lost: &mut self.lost }, &mut |hop| {
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
        let asking = self.stack.next_timer().map(|ms| ms.div_ceil(MS_PER_TICK));
        let pings = self.pings.iter().flatten().map(|ping| ping.deadline).min();
        asking.into_iter().chain(pings).min().unwrap_or(u64::MAX)
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
        let mut wire = Card { card, lost: &mut self.lost };
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
                    let mut wire = Card { card, lost: &mut self.lost };
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
        pings: [None; PINGS],
        identifier: taaj::process_id() as u16,
        next_port: FIRST_FREE_PORT,
        no_such_port: 0,
        not_taken: 0,
        lost: 0,
    };

    let mut frame = [0u8; abi::nic::FRAME_MAX];
    let mut message = [0u8; abi::MESSAGE_MAX as usize];
    loop {
        service.tick(&mut card, taaj::ticks());

        // The card, a request, or a socket - or the clock.
        let mut watched = [0u64; 2 + SOCKETS];
        let mut which = [0usize; SOCKETS];
        watched[0] = frames.channel();
        watched[1] = requests;
        let mut count = 2;
        for (n, socket) in service.sockets.iter().enumerate() {
            if let Some(socket) = socket {
                watched[count] = socket.channel;
                which[count - 2] = n;
                count += 1;
            }
        }
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
            n => service.socket(which[n - 2], &mut card, now),
        }
    }
}
