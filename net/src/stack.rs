//! What one machine on a network does: the stack.
//!
//! It is handed every frame the card received (`receive`) and the time
//! (`tick`), and is asked to send things (`send_udp`, `send_echo`) and
//! to make connections (`tcp_connect`, `tcp_listen`).
//! Frames to put on the wire go out through a `Wire`; what arrived for
//! the programs above comes back as a `Received`. No card, no clock
//! and no channel in here: a host test plays the whole network.
//!
//! What it does by itself, without anyone above knowing:
//!
//!   - answers "who has my address?" (ARP), and asks that of others
//!     before it sends to them, remembering the answers for a minute;
//!   - holds a packet back while it asks, and sends it when the answer
//!     comes - or drops it after three questions and three seconds;
//!   - answers pings;
//!   - decides where a packet goes: to the machine itself if it is on
//!     this wire, to the gateway (a router) if it is not;
//!   - keeps the TCP connections (each one is a `tcp::Connection`):
//!     finds the one a segment belongs to, makes a new one when a SYN
//!     arrives for a port that is listened on, says "no such
//!     connection" (a RST) to everything else, and writes the headers
//!     of what the connections have to send.
//!
//! Time is a number of milliseconds from anywhere, only ever compared.

use alloc::vec::Vec;

use crate::tcp::{Buffers, Connection, State};
use crate::wire::{arp, ethernet, icmp, ipv4, tcp, udp, Bad};
use crate::{Ipv4, Mac, EVERYONE};

/// The longest frame: 14 bytes of Ethernet, 1500 of packet.
pub const FRAME_MAX: usize = 1514;
const IP_AT: usize = ethernet::HEADER;
const INNER_AT: usize = IP_AT + ipv4::HEADER;
/// The most data one UDP datagram carries without being cut up on the
/// way (which nothing here does).
pub const UDP_PAYLOAD_MAX: usize = FRAME_MAX - INNER_AT - udp::HEADER;
/// And one ping.
pub const ECHO_DATA_MAX: usize = FRAME_MAX - INNER_AT - icmp::HEADER;

/// How often "who has this address?" is asked before giving up, and
/// how long each answer is waited for.
pub const ARP_TRIES: u8 = 3;
pub const ARP_RETRY_MS: u64 = 1000;
/// How long an answer is believed. Cards are replaced and addresses
/// move: after this, the question is asked again.
pub const ARP_KEEP_MS: u64 = 60_000;

/// How many TCP connections there can be at once, how many ports can
/// be listened on, and how many connections to one such port may be
/// waiting - half made, or made and not yet taken by its program.
pub const TCP_CONNECTIONS: usize = 16;
pub const TCP_LISTENERS: usize = 8;
pub const TCP_BACKLOG: usize = 4;

/// How many other machines' card addresses are remembered at once.
const NEIGHBOURS: usize = 16;
/// How many packets are held back, waiting for a card's address.
const HELD: usize = 4;

/// Who this machine is on its network.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Config {
    pub mac: Mac,
    pub address: Ipv4,
    /// How many leading bits of the address name the network: with
    /// 24, every 10.0.2.x is a neighbour.
    pub prefix: u8,
    /// The router: where packets for everywhere else go. `Ipv4::NONE`
    /// if there is none.
    pub gateway: Ipv4,
}

/// Where frames go out.
pub trait Wire {
    fn send(&mut self, frame: &[u8]);
}

impl<F: FnMut(&[u8])> Wire for F {
    fn send(&mut self, frame: &[u8]) {
        self(frame)
    }
}

/// What became of a frame that was received.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Received<'a> {
    /// Dealt with in here: an ARP packet, a ping that was answered.
    Handled,
    /// A datagram for a program. If no program has `to_port`, the
    /// sender should be told so: `Stack::refuse_udp`, with `packet`.
    Udp { from: Ipv4, from_port: u16, to_port: u16, payload: &'a [u8], packet: &'a [u8] },
    /// The answer to a ping sent from here.
    EchoReply { from: Ipv4, identifier: u16, sequence: u16, ttl: u8, data: &'a [u8] },
    Dropped(Why),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Why {
    /// Too short, or lengths that do not agree.
    Malformed,
    BadChecksum,
    /// For another machine.
    NotForUs,
    /// A piece of a packet cut up on the way: not put together here.
    Fragment,
    /// A protocol, or a kind of message, nothing here speaks.
    Unsupported,
}

/// What became of something to send.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sent {
    /// On the wire.
    Sent,
    /// Held back until the card that has the address is known.
    Waiting,
    /// Too many packets are being held back already: dropped.
    NoRoom,
    /// More than a frame carries.
    TooBig,
    /// Not on this network, and there is no gateway. (Or this machine
    /// itself: nothing here delivers to itself.)
    NoRoute,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Counters {
    pub frames_in: u64,
    pub frames_out: u64,
    /// "Who has my address?" answered.
    pub arp_answered: u64,
    pub echoes_answered: u64,
    pub datagrams_in: u64,
    /// Received and not understood: malformed, a wrong checksum.
    pub bad: u64,
    pub not_for_us: u64,
    /// Fragments, and protocols not spoken.
    pub unsupported: u64,
    /// Packets dropped because nobody had the address, or too many
    /// were waiting.
    pub undelivered: u64,
    /// TCP segments that arrived whole.
    pub segments_in: u64,
    /// Segments sent a second time.
    pub segments_again: u64,
    /// Segments answered "no such connection here".
    pub refused: u64,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Neighbour {
    Free,
    /// Asked for `tries` times; to be asked again (or given up) at `next`.
    Asking { ip: Ipv4, tries: u8, next: u64 },
    Known { ip: Ipv4, mac: Mac, until: u64 },
}

impl Neighbour {
    fn ip(&self) -> Option<Ipv4> {
        match *self {
            Neighbour::Free => None,
            Neighbour::Asking { ip, .. } | Neighbour::Known { ip, .. } => Some(ip),
        }
    }
}

/// A packet waiting for a card's address: the whole frame, all but
/// its first six bytes.
struct Held {
    used: bool,
    next_hop: Ipv4,
    length: usize,
    frame: [u8; FRAME_MAX],
}

enum Route {
    Everyone,
    Via(Ipv4),
    None,
}

/// A connection, and whose it is.
struct Slot {
    connection: Connection,
    local_port: u16,
    remote: Ipv4,
    remote_port: u16,
    /// It came in on a port that is listened on, and no program has
    /// taken it yet (`tcp_accept`).
    waiting: bool,
    /// Its program is done with it: forgotten as soon as it is closed.
    released: bool,
}

pub struct Stack {
    config: Config,
    neighbours: [Neighbour; NEIGHBOURS],
    held: [Held; HELD],
    /// The frame being built.
    out: [u8; FRAME_MAX],
    /// A number for each packet sent (for putting fragments together,
    /// which matters to others even if not here).
    identification: u16,
    counters: Counters,
    /// The connections, by their number (`TCP_CONNECTIONS` places).
    connections: Vec<Option<Slot>>,
    /// The ports listened on; 0 is a free place.
    listening: [u16; TCP_LISTENERS],
    /// What the connections' first numbers are made from (`seed`).
    secret: u32,
}

impl Stack {
    pub fn new(config: Config) -> Stack {
        Stack {
            config,
            neighbours: [Neighbour::Free; NEIGHBOURS],
            held: core::array::from_fn(|_| Held { used: false, next_hop: Ipv4::NONE, length: 0, frame: [0; FRAME_MAX] }),
            out: [0; FRAME_MAX],
            identification: 0,
            counters: Counters::default(),
            connections: (0..TCP_CONNECTIONS).map(|_| None).collect(),
            listening: [0; TCP_LISTENERS],
            secret: 0,
        }
    }

    /// A number nobody outside should know: where each connection's
    /// numbering starts is worked out from it.
    pub fn seed(&mut self, secret: u32) {
        self.secret = secret;
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn counters(&self) -> &Counters {
        &self.counters
    }

    fn is_broadcast(&self, address: Ipv4) -> bool {
        address == Ipv4::BROADCAST || (self.config.prefix < 31 && address == self.config.address.network_broadcast(self.config.prefix))
    }

    fn route(&self, to: Ipv4) -> Route {
        if to == Ipv4::NONE || to == self.config.address {
            Route::None
        } else if self.is_broadcast(to) {
            Route::Everyone
        } else if self.config.address.same_network(to, self.config.prefix) {
            Route::Via(to)
        } else if self.config.gateway != Ipv4::NONE {
            Route::Via(self.config.gateway)
        } else {
            Route::None
        }
    }

    /// The machine on this wire that a packet for `to` is handed to:
    /// `to` itself, or the gateway. None for everyone, and for nowhere.
    pub fn next_hop(&self, to: Ipv4) -> Option<Ipv4> {
        match self.route(to) {
            Route::Via(hop) => Some(hop),
            _ => None,
        }
    }

    /// The card that has `ip`, if that is known (and still believed).
    pub fn neighbour(&self, ip: Ipv4, now: u64) -> Option<Mac> {
        self.neighbours.iter().find_map(|neighbour| match *neighbour {
            Neighbour::Known { ip: known, mac, until } if known == ip && now < until => Some(mac),
            _ => None,
        })
    }

    /// When `tick` next has something to do, if ever.
    pub fn next_timer(&self) -> Option<u64> {
        let asking = self.neighbours.iter().filter_map(|neighbour| match *neighbour {
            Neighbour::Asking { next, .. } => Some(next),
            _ => None,
        });
        let connections = self.connections.iter().flatten().filter_map(|slot| slot.connection.next_timer());
        asking.chain(connections).min()
    }

    fn put(&mut self, frame: &[u8], wire: &mut dyn Wire) {
        self.counters.frames_out += 1;
        wire.send(frame);
    }

    fn ask(&mut self, ip: Ipv4, wire: &mut dyn Wire) {
        let mut frame = [0u8; ethernet::HEADER + arp::SIZE];
        ethernet::write(&mut frame, EVERYONE, self.config.mac, ethernet::TYPE_ARP);
        let question = arp::Packet { operation: arp::REQUEST, sender_mac: self.config.mac, sender_ip: self.config.address, target_mac: [0; 6], target_ip: ip };
        question.write(&mut frame[ethernet::HEADER..]);
        self.put(&frame, wire);
    }

    /// Notes that `ip` is at `mac`. An address being asked for, or
    /// known already, is always updated; a new one is added only if
    /// `add` (it spoke to this machine: an answer will be wanted).
    fn learn(&mut self, ip: Ipv4, mac: Mac, add: bool, now: u64, wire: &mut dyn Wire) {
        // Not addresses a machine can have: nothing to remember.
        if ip == Ipv4::NONE || self.is_broadcast(ip) || mac[0] & 1 != 0 {
            return;
        }
        let known = Neighbour::Known { ip, mac, until: now + ARP_KEEP_MS };
        if let Some(slot) = self.neighbours.iter().position(|neighbour| neighbour.ip() == Some(ip)) {
            let was_asking = matches!(self.neighbours[slot], Neighbour::Asking { .. });
            self.neighbours[slot] = known;
            if was_asking {
                self.release(ip, mac, wire);
            }
            return;
        }
        if !add {
            return;
        }
        // A free place; or the place of whoever was heard of longest
        // ago. (Never of one being asked for: packets wait on it.)
        let oldest = |neighbour: &Neighbour| match *neighbour {
            Neighbour::Free => Some(0),
            Neighbour::Known { until, .. } => Some(until.max(1)),
            Neighbour::Asking { .. } => None,
        };
        let slot = (0..NEIGHBOURS).filter(|&n| oldest(&self.neighbours[n]).is_some()).min_by_key(|&n| oldest(&self.neighbours[n]));
        if let Some(slot) = slot {
            self.neighbours[slot] = known;
        }
    }

    /// Sends every packet that was waiting for `next_hop`'s card.
    fn release(&mut self, next_hop: Ipv4, mac: Mac, wire: &mut dyn Wire) {
        for n in 0..HELD {
            if self.held[n].used && self.held[n].next_hop == next_hop {
                self.held[n].used = false;
                self.held[n].frame[..6].copy_from_slice(&mac);
                self.counters.frames_out += 1;
                wire.send(&self.held[n].frame[..self.held[n].length]);
            }
        }
    }

    /// One frame from the card.
    pub fn receive<'a>(&mut self, frame: &'a [u8], now: u64, wire: &mut dyn Wire) -> Received<'a> {
        self.counters.frames_in += 1;
        let received = match ethernet::parse(frame) {
            Some(frame) if frame.kind == ethernet::TYPE_ARP => self.receive_arp(frame.payload, now, wire),
            Some(frame) if frame.kind == ethernet::TYPE_IPV4 => self.receive_ip(frame.payload, now, wire),
            Some(_) => Received::Dropped(Why::Unsupported),
            None => Received::Dropped(Why::Malformed),
        };
        match received {
            Received::Dropped(Why::Malformed | Why::BadChecksum) => self.counters.bad += 1,
            Received::Dropped(Why::NotForUs) => self.counters.not_for_us += 1,
            Received::Dropped(Why::Fragment | Why::Unsupported) => self.counters.unsupported += 1,
            _ => {}
        }
        received
    }

    fn receive_arp<'a>(&mut self, payload: &[u8], now: u64, wire: &mut dyn Wire) -> Received<'a> {
        let Some(packet) = arp::parse(payload) else {
            return Received::Dropped(Why::Malformed);
        };
        // Someone else saying they have this machine's address: not
        // believed, and not answered.
        if packet.sender_ip == self.config.address {
            return Received::Dropped(Why::NotForUs);
        }
        let for_us = packet.target_ip == self.config.address;
        self.learn(packet.sender_ip, packet.sender_mac, for_us, now, wire);
        if !for_us {
            return Received::Dropped(Why::NotForUs);
        }
        if packet.operation == arp::REQUEST {
            let mut frame = [0u8; ethernet::HEADER + arp::SIZE];
            ethernet::write(&mut frame, packet.sender_mac, self.config.mac, ethernet::TYPE_ARP);
            let answer = arp::Packet {
                operation: arp::REPLY,
                sender_mac: self.config.mac,
                sender_ip: self.config.address,
                target_mac: packet.sender_mac,
                target_ip: packet.sender_ip,
            };
            answer.write(&mut frame[ethernet::HEADER..]);
            self.counters.arp_answered += 1;
            self.put(&frame, wire);
        }
        Received::Handled
    }

    fn receive_ip<'a>(&mut self, packet: &'a [u8], now: u64, wire: &mut dyn Wire) -> Received<'a> {
        let (header, payload) = match ipv4::parse(packet) {
            Ok(parsed) => parsed,
            Err(Bad::Checksum) => return Received::Dropped(Why::BadChecksum),
            Err(_) => return Received::Dropped(Why::Malformed),
        };
        let to_everyone = self.is_broadcast(header.destination);
        if header.destination != self.config.address && !to_everyone {
            return Received::Dropped(Why::NotForUs);
        }
        if header.fragment {
            return Received::Dropped(Why::Fragment);
        }
        let packet = &packet[..header.header_length + payload.len()];
        match header.protocol {
            ipv4::PROTOCOL_ICMP => {
                let message = match icmp::parse(payload) {
                    Ok(message) => message,
                    Err(Bad::Checksum) => return Received::Dropped(Why::BadChecksum),
                    Err(_) => return Received::Dropped(Why::Malformed),
                };
                match message.kind {
                    // A ping to everyone is not answered: a way to make
                    // a whole network shout at one address.
                    icmp::ECHO_REQUEST if !to_everyone && message.data.len() <= ECHO_DATA_MAX => {
                        self.out[INNER_AT + icmp::HEADER..][..message.data.len()].copy_from_slice(message.data);
                        let length = icmp::write(&mut self.out[INNER_AT..], icmp::ECHO_REPLY, 0, message.rest, message.data.len());
                        self.counters.echoes_answered += 1;
                        self.dispatch(header.source, ipv4::PROTOCOL_ICMP, length, now, wire);
                        Received::Handled
                    }
                    icmp::ECHO_REPLY => Received::EchoReply {
                        from: header.source,
                        identifier: message.identifier(),
                        sequence: message.sequence(),
                        ttl: header.ttl,
                        data: message.data,
                    },
                    _ => Received::Dropped(Why::Unsupported),
                }
            }
            ipv4::PROTOCOL_TCP => match tcp::parse(header.source, header.destination, payload) {
                // (A connection is between two machines: nothing sent
                // to everyone can belong to one.)
                Ok(_) if to_everyone => Received::Dropped(Why::NotForUs),
                Ok(segment) => {
                    self.counters.segments_in += 1;
                    self.receive_tcp(header.source, &segment, now, wire);
                    Received::Handled
                }
                Err(Bad::Checksum) => Received::Dropped(Why::BadChecksum),
                Err(_) => Received::Dropped(Why::Malformed),
            },
            ipv4::PROTOCOL_UDP => match udp::parse(header.source, header.destination, payload) {
                Ok(datagram) => {
                    self.counters.datagrams_in += 1;
                    Received::Udp { from: header.source, from_port: datagram.source_port, to_port: datagram.destination_port, payload: datagram.payload, packet }
                }
                Err(Bad::Checksum) => Received::Dropped(Why::BadChecksum),
                Err(_) => Received::Dropped(Why::Malformed),
            },
            _ => Received::Dropped(Why::Unsupported),
        }
    }

    /// Sends what is in `self.out` from `INNER_AT` on - `inner` bytes
    /// of ICMP or UDP - to `to`: writes the two headers before it, and
    /// either puts the frame on the wire or holds it back and asks who
    /// has the address.
    fn dispatch(&mut self, to: Ipv4, protocol: u8, inner: usize, now: u64, wire: &mut dyn Wire) -> Sent {
        let length = INNER_AT + inner;
        self.identification = self.identification.wrapping_add(1);
        ipv4::write(&mut self.out[IP_AT..], protocol, self.config.address, to, self.identification, inner);
        ethernet::write(&mut self.out, [0; 6], self.config.mac, ethernet::TYPE_IPV4);
        let next_hop = match self.route(to) {
            Route::Everyone => {
                self.out[..6].copy_from_slice(&EVERYONE);
                self.counters.frames_out += 1;
                wire.send(&self.out[..length]);
                return Sent::Sent;
            }
            Route::Via(hop) => hop,
            Route::None => {
                self.counters.undelivered += 1;
                return Sent::NoRoute;
            }
        };
        if let Some(mac) = self.neighbour(next_hop, now) {
            self.out[..6].copy_from_slice(&mac);
            self.counters.frames_out += 1;
            wire.send(&self.out[..length]);
            return Sent::Sent;
        }
        let Some(place) = self.held.iter().position(|held| !held.used) else {
            self.counters.undelivered += 1;
            return Sent::NoRoom;
        };
        let held = &mut self.held[place];
        held.used = true;
        held.next_hop = next_hop;
        held.length = length;
        held.frame[..length].copy_from_slice(&self.out[..length]);
        // Already being asked for? Then the answer will release this
        // one too. Otherwise ask now - in the place of what was known
        // of it and is no longer believed, or in a free one.
        if !self.neighbours.iter().any(|neighbour| matches!(neighbour, Neighbour::Asking { ip, .. } if *ip == next_hop)) {
            let asking = Neighbour::Asking { ip: next_hop, tries: 1, next: now + ARP_RETRY_MS };
            let slot = self
                .neighbours
                .iter()
                .position(|neighbour| neighbour.ip() == Some(next_hop))
                .or_else(|| self.neighbours.iter().position(|neighbour| *neighbour == Neighbour::Free))
                .or_else(|| {
                    // Full: the place of the one heard of longest ago.
                    (0..NEIGHBOURS)
                        .filter_map(|n| match self.neighbours[n] {
                            Neighbour::Known { until, .. } => Some((until, n)),
                            _ => None,
                        })
                        .min()
                        .map(|(_, n)| n)
                });
            match slot {
                Some(slot) => {
                    self.neighbours[slot] = asking;
                    self.ask(next_hop, wire);
                }
                // Sixteen addresses being asked for at once: this one
                // cannot be.
                None => {
                    self.held[place].used = false;
                    self.counters.undelivered += 1;
                    return Sent::NoRoom;
                }
            }
        }
        Sent::Waiting
    }

    /// A datagram from port `from_port` here to `to_port` on `to`.
    pub fn send_udp(&mut self, from_port: u16, to: Ipv4, to_port: u16, payload: &[u8], now: u64, wire: &mut dyn Wire) -> Sent {
        if payload.len() > UDP_PAYLOAD_MAX {
            return Sent::TooBig;
        }
        self.out[INNER_AT + udp::HEADER..][..payload.len()].copy_from_slice(payload);
        let length = udp::write(&mut self.out[INNER_AT..], self.config.address, to, from_port, to_port, payload.len());
        self.dispatch(to, ipv4::PROTOCOL_UDP, length, now, wire)
    }

    /// A ping: `to` should send `data` back, with the same two numbers.
    pub fn send_echo(&mut self, to: Ipv4, identifier: u16, sequence: u16, data: &[u8], now: u64, wire: &mut dyn Wire) -> Sent {
        if data.len() > ECHO_DATA_MAX {
            return Sent::TooBig;
        }
        self.out[INNER_AT + icmp::HEADER..][..data.len()].copy_from_slice(data);
        let [i0, i1] = identifier.to_be_bytes();
        let [s0, s1] = sequence.to_be_bytes();
        let length = icmp::write(&mut self.out[INNER_AT..], icmp::ECHO_REQUEST, 0, [i0, i1, s0, s1], data.len());
        self.dispatch(to, ipv4::PROTOCOL_ICMP, length, now, wire)
    }

    /// Tells the sender of a datagram that no program here has its
    /// port ("destination unreachable: port"). `packet` is the one a
    /// `Received::Udp` carried. The message quotes the packet's header
    /// and its first eight bytes - the ports - so that the sender can
    /// tell which of its datagrams is meant.
    pub fn refuse_udp(&mut self, packet: &[u8], now: u64, wire: &mut dyn Wire) {
        let Ok((header, payload)) = ipv4::parse(packet) else { return };
        // Only what was sent to this machine alone: an answer to a
        // broadcast would be one from every machine on the wire.
        if header.destination != self.config.address {
            return;
        }
        let quoted = header.header_length + payload.len().min(8);
        self.out[INNER_AT + icmp::HEADER..][..quoted].copy_from_slice(&packet[..quoted]);
        let length = icmp::write(&mut self.out[INNER_AT..], icmp::UNREACHABLE, icmp::CODE_PORT, [0; 4], quoted);
        self.dispatch(header.source, ipv4::PROTOCOL_ICMP, length, now, wire);
    }

    /// The clock. Asks again for addresses nobody has answered for,
    /// and gives up on those asked for often enough: the packets
    /// waiting for them are dropped, and `gave_up` is told the address
    /// (a next hop: see `next_hop`).
    pub fn tick(&mut self, now: u64, wire: &mut dyn Wire, gave_up: &mut dyn FnMut(Ipv4)) {
        for n in 0..NEIGHBOURS {
            match self.neighbours[n] {
                Neighbour::Asking { ip, tries, next } if now >= next => {
                    if tries >= ARP_TRIES {
                        self.neighbours[n] = Neighbour::Free;
                        for held in self.held.iter_mut().filter(|held| held.used && held.next_hop == ip) {
                            held.used = false;
                            self.counters.undelivered += 1;
                        }
                        // A connection being made to there, or by way
                        // of there, will never be.
                        for m in 0..TCP_CONNECTIONS {
                            if let Some(remote) = self.connections[m].as_ref().map(|slot| slot.remote) {
                                if self.next_hop(remote) == Some(ip) {
                                    if let Some(slot) = self.connections[m].as_mut() {
                                        slot.connection.unreachable();
                                    }
                                }
                            }
                        }
                        gave_up(ip);
                    } else {
                        self.neighbours[n] = Neighbour::Asking { ip, tries: tries + 1, next: now + ARP_RETRY_MS };
                        self.ask(ip, wire);
                    }
                }
                Neighbour::Known { until, .. } if now >= until => self.neighbours[n] = Neighbour::Free,
                _ => {}
            }
        }
        for n in 0..TCP_CONNECTIONS {
            if let Some(slot) = self.connections[n].as_mut() {
                slot.connection.tick(now);
                self.tcp_flush(n, now, wire);
            }
        }
    }

    // TCP.

    /// Where a new connection's numbering starts: different for every
    /// pair of ends, moving on with the clock (so that a connection
    /// made again between the same ends does not take the old one's
    /// late segments for its own), and not to be guessed without
    /// `secret`. (RFC 6528 asks for a real cryptographic hash here;
    /// this is a small mixing function, and the secret is only as
    /// secret as whoever calls `seed` can make it.)
    fn first_number(&self, local_port: u16, remote: Ipv4, remote_port: u16, now: u64) -> u32 {
        let mut mixed = self.secret ^ 0x9E37_79B9;
        for word in [remote.to_u32(), self.config.address.to_u32(), u32::from(local_port) << 16 | u32::from(remote_port)] {
            mixed = (mixed ^ word).wrapping_mul(0x85EB_CA6B);
            mixed ^= mixed >> 13;
            mixed = mixed.wrapping_mul(0xC2B2_AE35);
            mixed ^= mixed >> 16;
        }
        // One step every four microseconds, as the standard has it.
        mixed.wrapping_add((now as u32).wrapping_mul(250))
    }

    fn is_listening(&self, port: u16) -> bool {
        port != 0 && self.listening.contains(&port)
    }

    /// The connection between `local_port` here and `remote_port` on
    /// `remote`, if there is one (and it has not ended).
    fn tcp_find(&self, local_port: u16, remote: Ipv4, remote_port: u16) -> Option<usize> {
        self.connections.iter().position(|slot| {
            slot.as_ref().is_some_and(|slot| {
                slot.local_port == local_port && slot.remote == remote && slot.remote_port == remote_port && slot.connection.state() != State::Closed
            })
        })
    }

    /// Nothing is using `port` on this machine: no connection, and
    /// nobody listening.
    pub fn tcp_port_free(&self, port: u16) -> bool {
        !self.is_listening(port) && !self.connections.iter().flatten().any(|slot| slot.local_port == port)
    }

    /// Connections to `port` are accepted from now on (`tcp_accept`
    /// hands them out). False if the port is in use, or too many are
    /// listened on.
    pub fn tcp_listen(&mut self, port: u16) -> bool {
        if port == 0 || !self.tcp_port_free(port) {
            return false;
        }
        match self.listening.iter_mut().find(|listened| **listened == 0) {
            Some(place) => {
                *place = port;
                true
            }
            None => false,
        }
    }

    /// No more connections to `port`; those nobody took are cut off.
    pub fn tcp_unlisten(&mut self, port: u16, now: u64, wire: &mut dyn Wire) {
        for listened in self.listening.iter_mut().filter(|listened| **listened == port) {
            *listened = 0;
        }
        for n in 0..TCP_CONNECTIONS {
            if let Some(slot) = self.connections[n].as_mut().filter(|slot| slot.waiting && slot.local_port == port) {
                slot.connection.abort();
                self.tcp_flush(n, now, wire);
            }
        }
    }

    /// A connection to `port` that has been made and not handed out
    /// yet: its number, and who it is with. It is the caller's from
    /// now on, until `tcp_close`.
    pub fn tcp_accept(&mut self, port: u16) -> Option<(usize, Ipv4, u16)> {
        let n = self.connections.iter().position(|slot| {
            slot.as_ref().is_some_and(|slot| {
                slot.waiting && slot.local_port == port && !matches!(slot.connection.state(), State::SynReceived | State::Closed)
            })
        })?;
        let slot = self.connections[n].as_mut()?;
        slot.waiting = false;
        Some((n, slot.remote, slot.remote_port))
    }

    /// Starts a connection from `local_port` (which must be free:
    /// `tcp_port_free`) to `remote_port` on `remote`: its number, to
    /// ask about with `tcp`. `NoRoute` if no packet can be sent there,
    /// `NoRoom` if there are too many connections already.
    pub fn tcp_connect(&mut self, local_port: u16, remote: Ipv4, remote_port: u16, now: u64, wire: &mut dyn Wire) -> Result<usize, Sent> {
        if self.next_hop(remote).is_none() || remote_port == 0 {
            return Err(Sent::NoRoute);
        }
        let n = self.connections.iter().position(Option::is_none).ok_or(Sent::NoRoom)?;
        let connection = Connection::connect(self.first_number(local_port, remote, remote_port, now), Buffers::default());
        self.connections[n] = Some(Slot { connection, local_port, remote, remote_port, waiting: false, released: false });
        self.tcp_flush(n, now, wire);
        Ok(n)
    }

    /// Connection `n`, to ask how it is: its state, why it failed, how
    /// much can be read or written.
    pub fn tcp(&self, n: usize) -> Option<&Connection> {
        self.connections.get(n)?.as_ref().map(|slot| &slot.connection)
    }

    /// Takes as much of `data` as connection `n` has room for, and
    /// sends what can be sent; how much was taken.
    pub fn tcp_write(&mut self, n: usize, data: &[u8], now: u64, wire: &mut dyn Wire) -> usize {
        let Some(slot) = self.connections.get_mut(n).and_then(Option::as_mut) else { return 0 };
        let taken = slot.connection.write(data);
        self.tcp_flush(n, now, wire);
        taken
    }

    /// Copies what has arrived on connection `n` to `out`, without
    /// taking it; how much.
    pub fn tcp_peek(&self, n: usize, out: &mut [u8]) -> usize {
        self.tcp(n).map_or(0, |connection| connection.peek(out))
    }

    /// Takes `count` bytes that `tcp_peek` showed.
    pub fn tcp_consume(&mut self, n: usize, count: usize, now: u64, wire: &mut dyn Wire) {
        if let Some(slot) = self.connections.get_mut(n).and_then(Option::as_mut) {
            slot.connection.consume(count);
            self.tcp_flush(n, now, wire);
        }
    }

    /// Connection `n` has no more to send (the other direction stays
    /// open).
    pub fn tcp_shutdown(&mut self, n: usize, now: u64, wire: &mut dyn Wire) {
        if let Some(slot) = self.connections.get_mut(n).and_then(Option::as_mut) {
            slot.connection.shutdown();
            self.tcp_flush(n, now, wire);
        }
    }

    /// The program is done with connection `n`: it is closed in order
    /// (or cut off, if something was left unread), and its number is
    /// free for another once that is over.
    pub fn tcp_close(&mut self, n: usize, now: u64, wire: &mut dyn Wire) {
        if let Some(slot) = self.connections.get_mut(n).and_then(Option::as_mut) {
            slot.released = true;
            slot.connection.close(now);
            self.tcp_flush(n, now, wire);
        }
    }

    /// The program is done with connection `n` and wants nothing more
    /// of it: cut off (a RST), and forgotten.
    pub fn tcp_abort(&mut self, n: usize, now: u64, wire: &mut dyn Wire) {
        if let Some(slot) = self.connections.get_mut(n).and_then(Option::as_mut) {
            slot.released = true;
            slot.connection.abort();
            self.tcp_flush(n, now, wire);
        }
    }

    /// The port of this machine that connection `n` uses.
    pub fn tcp_local_port(&self, n: usize) -> Option<u16> {
        self.connections.get(n)?.as_ref().map(|slot| slot.local_port)
    }

    /// Sends everything connection `n` has to send now - and forgets
    /// the connection if it has ended and nobody will ask about it.
    fn tcp_flush(&mut self, n: usize, now: u64, wire: &mut dyn Wire) {
        const DATA_AT: usize = INNER_AT + tcp::HEADER;
        loop {
            let Some(slot) = self.connections[n].as_mut() else { return };
            let again = slot.connection.retransmitted();
            let Some(out) = slot.connection.poll(now, &mut self.out[DATA_AT..]) else { break };
            self.counters.segments_again += u64::from(slot.connection.retransmitted() - again);
            let (remote, described) = (
                slot.remote,
                tcp::Segment {
                    source_port: slot.local_port,
                    destination_port: slot.remote_port,
                    sequence: out.sequence,
                    acknowledgment: out.acknowledgment,
                    flags: out.flags,
                    window: out.window,
                    mss: out.mss,
                    payload: &[],
                },
            );
            let length = tcp::write(&mut self.out[INNER_AT..], self.config.address, remote, &described, out.payload);
            // Not sent - held back for an address, or dropped for want
            // of room - is the same as lost on the way: the connection
            // sends it again.
            self.dispatch(remote, ipv4::PROTOCOL_TCP, length, now, wire);
        }
        if self.connections[n].as_ref().is_some_and(|slot| slot.connection.state() == State::Closed && (slot.released || slot.waiting)) {
            self.connections[n] = None;
        }
    }

    fn receive_tcp(&mut self, from: Ipv4, segment: &tcp::Segment<'_>, now: u64, wire: &mut dyn Wire) {
        if let Some(n) = self.tcp_find(segment.destination_port, from, segment.source_port) {
            if let Some(slot) = self.connections[n].as_mut() {
                slot.connection.segment(segment, now);
            }
            return self.tcp_flush(n, now, wire);
        }
        // No such connection. A SYN for a port that is listened on
        // makes one - if there is a place, and not too many are
        // waiting there already. If not, nothing is said: the other
        // side will ask again, and there may be room then.
        let opening = segment.flags & (tcp::SYN | tcp::ACK | tcp::RST | tcp::FIN) == tcp::SYN;
        if opening && self.is_listening(segment.destination_port) {
            let waiting = self.connections.iter().flatten().filter(|slot| slot.waiting && slot.local_port == segment.destination_port).count();
            let place = self.connections.iter().position(Option::is_none);
            if let (Some(n), true) = (place, waiting < TCP_BACKLOG) {
                let first = self.first_number(segment.destination_port, from, segment.source_port, now);
                self.connections[n] = Some(Slot {
                    connection: Connection::accept(first, segment, Buffers::default()),
                    local_port: segment.destination_port,
                    remote: from,
                    remote_port: segment.source_port,
                    waiting: true,
                    released: false,
                });
                self.tcp_flush(n, now, wire);
            }
            return;
        }
        // Everything else is answered "no such connection": a RST made
        // so that the other side believes it - with the number it
        // expects next (the one it acknowledged), or, if it
        // acknowledged nothing, acknowledging all of its segment.
        // Except a RST itself: two machines could say "no" to each
        // other for ever.
        if segment.has(tcp::RST) || from == Ipv4::NONE || self.is_broadcast(from) {
            return;
        }
        let (sequence, acknowledgment, flags) = if segment.has(tcp::ACK) {
            (segment.acknowledgment, 0, tcp::RST)
        } else {
            (0, segment.sequence.wrapping_add(segment.length()), tcp::RST | tcp::ACK)
        };
        let answer = tcp::Segment {
            source_port: segment.destination_port,
            destination_port: segment.source_port,
            sequence,
            acknowledgment,
            flags,
            window: 0,
            mss: None,
            payload: &[],
        };
        let length = tcp::write(&mut self.out[INNER_AT..], self.config.address, from, &answer, 0);
        self.counters.refused += 1;
        self.dispatch(from, ipv4::PROTOCOL_TCP, length, now, wire);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OUR_MAC: Mac = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];
    const OURS: Ipv4 = Ipv4([10, 0, 2, 15]);
    const GATEWAY: Ipv4 = Ipv4([10, 0, 2, 2]);
    const GATEWAY_MAC: Mac = [0x52, 0x55, 0x0a, 0x00, 0x02, 0x02];
    const PEER: Ipv4 = Ipv4([10, 0, 2, 7]);
    const PEER_MAC: Mac = [2, 0, 0, 0, 0, 7];
    const FAR: Ipv4 = Ipv4([93, 184, 216, 34]);

    fn stack() -> Stack {
        Stack::new(Config { mac: OUR_MAC, address: OURS, prefix: 24, gateway: GATEWAY })
    }

    /// The wire: every frame sent, kept.
    #[derive(Default)]
    struct Sink(Vec<Vec<u8>>);

    impl Wire for Sink {
        fn send(&mut self, frame: &[u8]) {
            self.0.push(frame.to_vec());
        }
    }

    // The other machines' side, written with the same `wire` functions
    // (which have tests of their own against bytes written by hand).

    fn arp_frame(operation: u16, from_mac: Mac, from: Ipv4, to_mac: Mac, target: Ipv4) -> Vec<u8> {
        let mut frame = vec![0u8; 60];
        ethernet::write(&mut frame, if operation == arp::REQUEST { EVERYONE } else { to_mac }, from_mac, ethernet::TYPE_ARP);
        arp::Packet { operation, sender_mac: from_mac, sender_ip: from, target_mac: if operation == arp::REQUEST { [0; 6] } else { to_mac }, target_ip: target }
            .write(&mut frame[14..]);
        frame
    }

    fn ip_frame(from_mac: Mac, from: Ipv4, to: Ipv4, protocol: u8, inner: &[u8]) -> Vec<u8> {
        let mut frame = vec![0u8; (34 + inner.len()).max(60)];
        ethernet::write(&mut frame, OUR_MAC, from_mac, ethernet::TYPE_IPV4);
        frame[34..34 + inner.len()].copy_from_slice(inner);
        ipv4::write(&mut frame[14..], protocol, from, to, 99, inner.len());
        frame
    }

    fn echo(kind: u8, identifier: u16, sequence: u16, data: &[u8]) -> Vec<u8> {
        let mut message = vec![0u8; 8 + data.len()];
        message[8..].copy_from_slice(data);
        let [i0, i1] = identifier.to_be_bytes();
        let [s0, s1] = sequence.to_be_bytes();
        icmp::write(&mut message, kind, 0, [i0, i1, s0, s1], data.len());
        message
    }

    fn datagram(from: Ipv4, to: Ipv4, from_port: u16, to_port: u16, payload: &[u8]) -> Vec<u8> {
        let mut segment = vec![0u8; 8 + payload.len()];
        segment[8..].copy_from_slice(payload);
        udp::write(&mut segment, from, to, from_port, to_port, payload.len());
        segment
    }

    /// What a frame the stack sent is, read back.
    fn read(frame: &[u8]) -> (ethernet::Frame<'_>, ipv4::Header, &[u8]) {
        let frame = ethernet::parse(frame).unwrap();
        assert_eq!(frame.kind, ethernet::TYPE_IPV4);
        assert_eq!(frame.from, OUR_MAC);
        let (header, payload) = ipv4::parse(frame.payload).unwrap();
        assert_eq!(header.source, OURS);
        (frame, header, payload)
    }

    fn is_question(frame: &[u8], about: Ipv4) -> bool {
        let Some(frame) = ethernet::parse(frame) else { return false };
        frame.kind == ethernet::TYPE_ARP
            && frame.to == EVERYONE
            && frame.from == OUR_MAC
            && arp::parse(frame.payload)
                == Some(arp::Packet { operation: arp::REQUEST, sender_mac: OUR_MAC, sender_ip: OURS, target_mac: [0; 6], target_ip: about })
    }

    #[test]
    fn who_has_my_address_is_answered() {
        let (mut stack, mut wire) = (stack(), Sink::default());
        let question = arp_frame(arp::REQUEST, PEER_MAC, PEER, [0; 6], OURS);
        assert_eq!(stack.receive(&question, 0, &mut wire), Received::Handled);
        assert_eq!(wire.0.len(), 1);
        let frame = ethernet::parse(&wire.0[0]).unwrap();
        // To the one who asked, not to everyone.
        assert_eq!((frame.to, frame.from, frame.kind), (PEER_MAC, OUR_MAC, ethernet::TYPE_ARP));
        assert_eq!(
            arp::parse(frame.payload),
            Some(arp::Packet { operation: arp::REPLY, sender_mac: OUR_MAC, sender_ip: OURS, target_mac: PEER_MAC, target_ip: PEER })
        );
        // And who asked is remembered: an answer to it needs no question.
        assert_eq!(stack.neighbour(PEER, 1), Some(PEER_MAC));
        assert_eq!(stack.counters().arp_answered, 1);
    }

    #[test]
    fn questions_about_others_are_not_answered_and_teach_nothing() {
        let (mut stack, mut wire) = (stack(), Sink::default());
        let question = arp_frame(arp::REQUEST, PEER_MAC, PEER, [0; 6], Ipv4([10, 0, 2, 9]));
        assert_eq!(stack.receive(&question, 0, &mut wire), Received::Dropped(Why::NotForUs));
        assert!(wire.0.is_empty());
        assert_eq!(stack.neighbour(PEER, 1), None, "a table of everyone who ever asked anything would be full at once");
        // Someone claiming this machine's own address is not believed.
        let question = arp_frame(arp::REQUEST, PEER_MAC, OURS, [0; 6], OURS);
        assert_eq!(stack.receive(&question, 0, &mut wire), Received::Dropped(Why::NotForUs));
        assert!(wire.0.is_empty());
        // Nor is an answer nobody asked for, about a broadcast address
        // or from a group's card address.
        stack.receive(&arp_frame(arp::REPLY, PEER_MAC, Ipv4([10, 0, 2, 255]), OUR_MAC, OURS), 0, &mut wire);
        stack.receive(&arp_frame(arp::REPLY, [1, 0, 0x5e, 0, 0, 1], Ipv4([10, 0, 2, 8]), OUR_MAC, OURS), 0, &mut wire);
        assert_eq!(stack.neighbour(Ipv4([10, 0, 2, 255]), 1), None);
        assert_eq!(stack.neighbour(Ipv4([10, 0, 2, 8]), 1), None);
    }

    #[test]
    fn a_ping_is_answered() {
        let (mut stack, mut wire) = (stack(), Sink::default());
        stack.receive(&arp_frame(arp::REQUEST, PEER_MAC, PEER, [0; 6], OURS), 0, &mut wire);
        wire.0.clear();
        for data in [&b""[..], b"x", b"abcdefgh", &[0xA5; 1472]] {
            let request = ip_frame(PEER_MAC, PEER, OURS, ipv4::PROTOCOL_ICMP, &echo(icmp::ECHO_REQUEST, 0xBEEF, 7, data));
            assert_eq!(stack.receive(&request, 5, &mut wire), Received::Handled);
            let sent = wire.0.pop().expect("an answer");
            let (frame, header, payload) = read(&sent);
            assert_eq!((frame.to, header.destination, header.protocol), (PEER_MAC, PEER, ipv4::PROTOCOL_ICMP));
            let message = icmp::parse(payload).unwrap();
            assert_eq!((message.kind, message.code, message.identifier(), message.sequence()), (icmp::ECHO_REPLY, 0, 0xBEEF, 7));
            assert_eq!(message.data, data);
        }
        assert_eq!(stack.counters().echoes_answered, 4);
    }

    #[test]
    fn a_ping_from_a_stranger_is_answered_once_its_card_is_known() {
        let (mut stack, mut wire) = (stack(), Sink::default());
        let request = ip_frame(PEER_MAC, PEER, OURS, ipv4::PROTOCOL_ICMP, &echo(icmp::ECHO_REQUEST, 1, 1, b"hi"));
        assert_eq!(stack.receive(&request, 0, &mut wire), Received::Handled);
        // Not taken from the frame it came in: asked for.
        assert_eq!(wire.0.len(), 1);
        assert!(is_question(&wire.0[0], PEER));
        stack.receive(&arp_frame(arp::REPLY, PEER_MAC, PEER, OUR_MAC, OURS), 10, &mut wire);
        assert_eq!(wire.0.len(), 2);
        let (frame, _, payload) = read(&wire.0[1]);
        assert_eq!(frame.to, PEER_MAC);
        assert_eq!(icmp::parse(payload).unwrap().data, b"hi");
    }

    #[test]
    fn a_ping_goes_out_after_the_question_and_its_answer_comes_back() {
        let (mut stack, mut wire) = (stack(), Sink::default());
        assert_eq!(stack.send_echo(PEER, 0x0102, 3, b"ping!", 0, &mut wire), Sent::Waiting);
        assert_eq!(wire.0.len(), 1);
        assert!(is_question(&wire.0[0], PEER));
        assert_eq!(stack.next_timer(), Some(ARP_RETRY_MS));

        assert_eq!(stack.receive(&arp_frame(arp::REPLY, PEER_MAC, PEER, OUR_MAC, OURS), 20, &mut wire), Received::Handled);
        assert_eq!(wire.0.len(), 2);
        let (frame, header, payload) = read(&wire.0[1]);
        assert_eq!((frame.to, header.destination, header.ttl), (PEER_MAC, PEER, 64));
        let message = icmp::parse(payload).unwrap();
        assert_eq!((message.kind, message.identifier(), message.sequence(), message.data), (icmp::ECHO_REQUEST, 0x0102, 3, &b"ping!"[..]));
        assert_eq!(stack.next_timer(), None);

        // The second needs no question.
        assert_eq!(stack.send_echo(PEER, 0x0102, 4, b"ping!", 30, &mut wire), Sent::Sent);
        assert_eq!(wire.0.len(), 3);

        let answer = ip_frame(PEER_MAC, PEER, OURS, ipv4::PROTOCOL_ICMP, &echo(icmp::ECHO_REPLY, 0x0102, 3, b"ping!"));
        assert_eq!(
            stack.receive(&answer, 40, &mut wire),
            Received::EchoReply { from: PEER, identifier: 0x0102, sequence: 3, ttl: 64, data: b"ping!" }
        );
        assert_eq!(wire.0.len(), 3, "an answer is not answered");
    }

    #[test]
    fn what_is_not_on_this_wire_goes_to_the_gateway() {
        let (mut stack, mut wire) = (stack(), Sink::default());
        assert_eq!(stack.next_hop(FAR), Some(GATEWAY));
        assert_eq!(stack.next_hop(PEER), Some(PEER));
        assert_eq!(stack.send_udp(4000, FAR, 53, b"far away", 0, &mut wire), Sent::Waiting);
        // The question is about the gateway, not about the far machine.
        assert!(is_question(&wire.0[0], GATEWAY));
        stack.receive(&arp_frame(arp::REPLY, GATEWAY_MAC, GATEWAY, OUR_MAC, OURS), 1, &mut wire);
        let (frame, header, payload) = read(&wire.0[1]);
        // The frame is for the gateway's card; the packet in it for the
        // far machine.
        assert_eq!((frame.to, header.destination), (GATEWAY_MAC, FAR));
        let datagram = udp::parse(OURS, FAR, payload).unwrap();
        assert_eq!((datagram.source_port, datagram.destination_port, datagram.payload), (4000, 53, &b"far away"[..]));
    }

    #[test]
    fn without_a_gateway_far_is_nowhere_and_so_is_this_machine() {
        let mut stack = Stack::new(Config { mac: OUR_MAC, address: OURS, prefix: 24, gateway: Ipv4::NONE });
        let mut wire = Sink::default();
        assert_eq!(stack.send_udp(1, FAR, 2, b"x", 0, &mut wire), Sent::NoRoute);
        assert_eq!(stack.send_udp(1, OURS, 2, b"x", 0, &mut wire), Sent::NoRoute);
        assert_eq!(stack.send_udp(1, Ipv4::NONE, 2, b"x", 0, &mut wire), Sent::NoRoute);
        assert!(wire.0.is_empty());
        assert_eq!(stack.counters().undelivered, 3);
    }

    #[test]
    fn a_question_is_asked_three_times_and_then_given_up() {
        let (mut stack, mut wire) = (stack(), Sink::default());
        let mut gone = Vec::new();
        assert_eq!(stack.send_echo(PEER, 1, 1, b"a", 0, &mut wire), Sent::Waiting);
        assert_eq!(stack.send_udp(9, PEER, 9, b"b", 5, &mut wire), Sent::Waiting);
        assert_eq!(wire.0.len(), 1, "one question for both");

        stack.tick(999, &mut wire, &mut |ip| gone.push(ip));
        assert_eq!(wire.0.len(), 1);
        stack.tick(1000, &mut wire, &mut |ip| gone.push(ip));
        assert_eq!(wire.0.len(), 2);
        assert_eq!(stack.next_timer(), Some(2000));
        stack.tick(2000, &mut wire, &mut |ip| gone.push(ip));
        assert_eq!(wire.0.len(), 3);
        assert!(wire.0.iter().all(|frame| is_question(frame, PEER)));
        assert!(gone.is_empty());

        stack.tick(3000, &mut wire, &mut |ip| gone.push(ip));
        assert_eq!(gone, [PEER]);
        assert_eq!(wire.0.len(), 3, "no fourth question");
        assert_eq!(stack.next_timer(), None);
        assert_eq!(stack.counters().undelivered, 2);

        // An answer that comes too late releases nothing: the packets
        // are gone.
        stack.receive(&arp_frame(arp::REPLY, PEER_MAC, PEER, OUR_MAC, OURS), 3500, &mut wire);
        assert_eq!(wire.0.len(), 3);
    }

    #[test]
    fn only_so_many_packets_wait_and_they_leave_in_order() {
        let (mut stack, mut wire) = (stack(), Sink::default());
        for n in 0..HELD as u8 {
            assert_eq!(stack.send_udp(100, PEER, 200, &[n], 0, &mut wire), Sent::Waiting);
        }
        assert_eq!(stack.send_udp(100, PEER, 200, &[99], 0, &mut wire), Sent::NoRoom);
        assert_eq!(wire.0.len(), 1);
        stack.receive(&arp_frame(arp::REPLY, PEER_MAC, PEER, OUR_MAC, OURS), 1, &mut wire);
        assert_eq!(wire.0.len(), 1 + HELD);
        for n in 0..HELD {
            let (frame, _, payload) = read(&wire.0[1 + n]);
            assert_eq!(frame.to, PEER_MAC);
            assert_eq!(udp::parse(OURS, PEER, payload).unwrap().payload, [n as u8]);
        }
        // And there is room again.
        assert_eq!(stack.send_udp(100, PEER, 200, &[5], 2, &mut wire), Sent::Sent);
        // For as many as before, once the address has to be asked for
        // again - and what left the first time does not leave twice.
        let later = ARP_KEEP_MS + 10;
        wire.0.clear();
        for n in 0..HELD as u8 {
            assert_eq!(stack.send_udp(100, PEER, 200, &[50 + n], later, &mut wire), Sent::Waiting);
        }
        stack.receive(&arp_frame(arp::REPLY, PEER_MAC, PEER, OUR_MAC, OURS), later + 1, &mut wire);
        assert_eq!(wire.0.len(), 1 + HELD);
        for n in 0..HELD {
            assert_eq!(udp::parse(OURS, PEER, read(&wire.0[1 + n]).2).unwrap().payload, [50 + n as u8]);
        }
    }

    #[test]
    fn what_is_known_is_asked_again_after_a_minute() {
        let (mut stack, mut wire) = (stack(), Sink::default());
        stack.receive(&arp_frame(arp::REQUEST, PEER_MAC, PEER, [0; 6], OURS), 0, &mut wire);
        wire.0.clear();
        assert_eq!(stack.send_udp(1, PEER, 2, b"x", ARP_KEEP_MS - 1, &mut wire), Sent::Sent);
        // Not believed any longer - with or without a tick in between.
        assert_eq!(stack.send_udp(1, PEER, 2, b"x", ARP_KEEP_MS, &mut wire), Sent::Waiting);
        assert!(is_question(&wire.0[1], PEER));
        // The card was replaced meanwhile: the new address is used.
        let new_mac = [2, 0, 0, 0, 0, 0x77];
        stack.receive(&arp_frame(arp::REPLY, new_mac, PEER, OUR_MAC, OURS), ARP_KEEP_MS + 5, &mut wire);
        assert_eq!(ethernet::parse(&wire.0[2]).unwrap().to, new_mac);

        // With a tick: forgotten.
        let (mut stack, mut wire) = (self::stack(), Sink::default());
        stack.receive(&arp_frame(arp::REQUEST, PEER_MAC, PEER, [0; 6], OURS), 0, &mut wire);
        stack.tick(ARP_KEEP_MS, &mut wire, &mut |_| panic!("nothing was being asked for"));
        assert_eq!(stack.neighbour(PEER, 0), None);
    }

    #[test]
    fn a_full_table_forgets_whoever_was_heard_of_longest_ago() {
        let (mut stack, mut wire) = (stack(), Sink::default());
        for n in 0..NEIGHBOURS as u8 {
            let (ip, mac) = (Ipv4([10, 0, 2, 100 + n]), [2, 0, 0, 0, 1, n]);
            stack.receive(&arp_frame(arp::REQUEST, mac, ip, [0; 6], OURS), u64::from(n), &mut wire);
        }
        assert_eq!(stack.neighbour(Ipv4([10, 0, 2, 100]), 20), Some([2, 0, 0, 0, 1, 0]));
        stack.receive(&arp_frame(arp::REQUEST, PEER_MAC, PEER, [0; 6], OURS), 20, &mut wire);
        assert_eq!(stack.neighbour(PEER, 20), Some(PEER_MAC));
        assert_eq!(stack.neighbour(Ipv4([10, 0, 2, 100]), 20), None, "the oldest made room");
        assert_eq!(stack.neighbour(Ipv4([10, 0, 2, 101]), 20), Some([2, 0, 0, 0, 1, 1]));
        // Asking for one more takes a place the same way.
        wire.0.clear();
        assert_eq!(stack.send_udp(1, Ipv4([10, 0, 2, 50]), 2, b"x", 21, &mut wire), Sent::Waiting);
        assert!(is_question(&wire.0[0], Ipv4([10, 0, 2, 50])));
        assert_eq!(stack.neighbour(Ipv4([10, 0, 2, 101]), 21), None);
    }

    #[test]
    fn a_datagram_arrives_or_is_dropped_for_a_reason() {
        let (mut stack, mut wire) = (stack(), Sink::default());
        let segment = datagram(PEER, OURS, 5000, 7, b"hello");
        let frame = ip_frame(PEER_MAC, PEER, OURS, ipv4::PROTOCOL_UDP, &segment);
        match stack.receive(&frame, 0, &mut wire) {
            Received::Udp { from, from_port, to_port, payload, packet } => {
                assert_eq!((from, from_port, to_port, payload), (PEER, 5000, 7, &b"hello"[..]));
                // The packet, without the frame's filling.
                assert_eq!(packet, &frame[14..14 + 20 + 13]);
            }
            other => panic!("{other:?}"),
        }
        assert!(wire.0.is_empty(), "receiving a datagram sends nothing");

        // To everyone, both ways of saying it.
        for to in [Ipv4::BROADCAST, Ipv4([10, 0, 2, 255])] {
            let frame = ip_frame(PEER_MAC, PEER, to, ipv4::PROTOCOL_UDP, &datagram(PEER, to, 68, 67, b"anyone?"));
            assert!(matches!(stack.receive(&frame, 0, &mut wire), Received::Udp { to_port: 67, .. }));
        }

        let mut wrong = frame.clone();
        wrong[45] ^= 1; // a byte of the data
        assert_eq!(stack.receive(&wrong, 0, &mut wire), Received::Dropped(Why::BadChecksum));
        let mut wrong = frame.clone();
        wrong[20] ^= 1; // a byte of the IPv4 header
        assert_eq!(stack.receive(&wrong, 0, &mut wire), Received::Dropped(Why::BadChecksum));

        let other = Ipv4([10, 0, 2, 16]);
        let frame = ip_frame(PEER_MAC, PEER, other, ipv4::PROTOCOL_UDP, &datagram(PEER, other, 1, 2, b"x"));
        assert_eq!(stack.receive(&frame, 0, &mut wire), Received::Dropped(Why::NotForUs));

        // A piece of a bigger packet.
        let mut piece = ip_frame(PEER_MAC, PEER, OURS, ipv4::PROTOCOL_UDP, &segment);
        piece[20] = 0x20; // "more fragments"
        piece[24..26].copy_from_slice(&[0, 0]);
        let sum = crate::checksum::of(&piece[14..34]);
        piece[24..26].copy_from_slice(&sum.to_be_bytes());
        assert_eq!(stack.receive(&piece, 0, &mut wire), Received::Dropped(Why::Fragment));

        // Protocols nothing here speaks: GRE (47), and IPv6 as a frame type.
        let frame = ip_frame(PEER_MAC, PEER, OURS, 47, &[0; 20]);
        assert_eq!(stack.receive(&frame, 0, &mut wire), Received::Dropped(Why::Unsupported));
        let mut frame = vec![0u8; 60];
        ethernet::write(&mut frame, OUR_MAC, PEER_MAC, 0x86DD);
        assert_eq!(stack.receive(&frame, 0, &mut wire), Received::Dropped(Why::Unsupported));

        assert!(wire.0.is_empty());
        let counters = stack.counters();
        assert_eq!((counters.datagrams_in, counters.bad, counters.not_for_us, counters.unsupported), (3, 2, 1, 3));
        assert_eq!(counters.frames_in, 9);
    }

    #[test]
    fn a_port_nobody_has_is_said_to_be_closed() {
        let (mut stack, mut wire) = (stack(), Sink::default());
        stack.receive(&arp_frame(arp::REQUEST, PEER_MAC, PEER, [0; 6], OURS), 0, &mut wire);
        wire.0.clear();
        let frame = ip_frame(PEER_MAC, PEER, OURS, ipv4::PROTOCOL_UDP, &datagram(PEER, OURS, 5000, 9999, b"anybody home?"));
        let Received::Udp { packet, .. } = stack.receive(&frame, 0, &mut wire) else { panic!() };
        let packet = packet.to_vec();
        stack.refuse_udp(&packet, 0, &mut wire);
        let (sent, header, payload) = read(&wire.0[0]);
        assert_eq!((sent.to, header.destination), (PEER_MAC, PEER));
        let message = icmp::parse(payload).unwrap();
        assert_eq!((message.kind, message.code, message.rest), (icmp::UNREACHABLE, icmp::CODE_PORT, [0; 4]));
        // The quote: the datagram's own header and its two ports.
        assert_eq!(message.data, &packet[..28]);

        // What was sent to everyone is not answered at all.
        let frame = ip_frame(PEER_MAC, PEER, Ipv4::BROADCAST, ipv4::PROTOCOL_UDP, &datagram(PEER, Ipv4::BROADCAST, 5000, 9999, b"x"));
        let Received::Udp { packet, .. } = stack.receive(&frame, 0, &mut wire) else { panic!() };
        let packet = packet.to_vec();
        stack.refuse_udp(&packet, 0, &mut wire);
        stack.refuse_udp(&[1, 2, 3], 0, &mut wire);
        assert_eq!(wire.0.len(), 1);
    }

    #[test]
    fn a_ping_to_everyone_is_not_answered() {
        let (mut stack, mut wire) = (stack(), Sink::default());
        let frame = ip_frame(PEER_MAC, PEER, Ipv4::BROADCAST, ipv4::PROTOCOL_ICMP, &echo(icmp::ECHO_REQUEST, 1, 1, b"all of you"));
        assert_eq!(stack.receive(&frame, 0, &mut wire), Received::Dropped(Why::Unsupported));
        // Nor is a ping whose checksum is wrong.
        let mut frame = ip_frame(PEER_MAC, PEER, OURS, ipv4::PROTOCOL_ICMP, &echo(icmp::ECHO_REQUEST, 1, 1, b"hm"));
        frame[43] ^= 4;
        assert_eq!(stack.receive(&frame, 0, &mut wire), Received::Dropped(Why::BadChecksum));
        assert!(wire.0.is_empty());
    }

    #[test]
    fn datagrams_go_out_as_they_should() {
        let (mut stack, mut wire) = (stack(), Sink::default());
        // To everyone: no question, the card address that means everyone.
        assert_eq!(stack.send_udp(68, Ipv4::BROADCAST, 67, b"discover", 0, &mut wire), Sent::Sent);
        let (frame, header, payload) = read(&wire.0[0]);
        assert_eq!((frame.to, header.destination, header.protocol), (EVERYONE, Ipv4::BROADCAST, ipv4::PROTOCOL_UDP));
        assert_eq!(udp::parse(OURS, Ipv4::BROADCAST, payload).unwrap().payload, b"discover");
        assert_eq!(stack.send_udp(68, Ipv4([10, 0, 2, 255]), 67, b"x", 0, &mut wire), Sent::Sent);
        assert_eq!(ethernet::parse(&wire.0[1]).unwrap().to, EVERYONE);

        // The longest, and one byte more.
        stack.receive(&arp_frame(arp::REQUEST, PEER_MAC, PEER, [0; 6], OURS), 0, &mut wire);
        wire.0.clear();
        let most = vec![0x5A; UDP_PAYLOAD_MAX];
        assert_eq!(stack.send_udp(1, PEER, 2, &most, 0, &mut wire), Sent::Sent);
        assert_eq!(wire.0[0].len(), FRAME_MAX);
        assert_eq!(udp::parse(OURS, PEER, read(&wire.0[0]).2).unwrap().payload, &most[..]);
        assert_eq!(stack.send_udp(1, PEER, 2, &vec![0; UDP_PAYLOAD_MAX + 1], 0, &mut wire), Sent::TooBig);
        assert_eq!(stack.send_echo(PEER, 1, 1, &vec![0; ECHO_DATA_MAX + 1], 0, &mut wire), Sent::TooBig);
        assert_eq!(wire.0.len(), 1);

        // Each packet has a number of its own.
        stack.send_udp(1, PEER, 2, b"a", 0, &mut wire);
        stack.send_udp(1, PEER, 2, b"b", 0, &mut wire);
        assert_ne!(read(&wire.0[1]).1.identification, read(&wire.0[2]).1.identification);
    }

    #[test]
    fn nothing_on_the_wire_can_make_it_fall_over() {
        // Every beginning of some real frames, and every one of them
        // with each byte changed in turn: whatever comes of it, it must
        // be an answer and not a panic (an index out of range, an
        // overflow - a test build checks both).
        let frames = [
            arp_frame(arp::REQUEST, PEER_MAC, PEER, [0; 6], OURS),
            ip_frame(PEER_MAC, PEER, OURS, ipv4::PROTOCOL_ICMP, &echo(icmp::ECHO_REQUEST, 1, 1, b"abcdefghij")),
            ip_frame(PEER_MAC, PEER, OURS, ipv4::PROTOCOL_UDP, &datagram(PEER, OURS, 1, 2, b"0123456789abcdefghijklmnopqrstuvwxyz")),
            ip_frame(PEER_MAC, PEER, OURS, ipv4::PROTOCOL_ICMP, &echo(icmp::ECHO_REPLY, 1, 1, b"")),
        ];
        let (mut stack, mut wire) = (stack(), Sink::default());
        let mut seen = 0u32;
        for frame in &frames {
            for length in 0..=frame.len() {
                if let Received::Udp { packet, .. } = stack.receive(&frame[..length], 0, &mut wire) {
                    let packet = packet.to_vec();
                    stack.refuse_udp(&packet, 0, &mut wire);
                }
                seen += 1;
            }
            for at in 0..frame.len() {
                for change in [0x01, 0x80, 0xFF] {
                    let mut wrong = frame.clone();
                    wrong[at] ^= change;
                    if let Received::Udp { packet, .. } = stack.receive(&wrong, 0, &mut wire) {
                        let packet = packet.to_vec();
                        stack.refuse_udp(&packet, 0, &mut wire);
                    }
                    seen += 1;
                }
            }
        }
        assert_eq!(stack.counters().frames_in, u64::from(seen));
        // Whatever was sent in answer is itself well-formed.
        for sent in &wire.0 {
            let frame = ethernet::parse(sent).unwrap();
            assert_eq!(frame.from, OUR_MAC);
            match frame.kind {
                ethernet::TYPE_ARP => assert!(arp::parse(frame.payload).is_some()),
                _ => assert!(ipv4::parse(frame.payload).is_ok()),
            }
        }
    }

    // TCP. The other machine's segments are written by hand here; what
    // the stack sends is read back with `tcp::parse`, which checks its
    // checksum.

    fn segment(from_port: u16, to_port: u16, sequence: u32, acknowledgment: u32, flags: u8, data: &[u8]) -> Vec<u8> {
        let mut bytes = vec![0u8; 20 + data.len()];
        bytes[20..].copy_from_slice(data);
        let described =
            tcp::Segment { source_port: from_port, destination_port: to_port, sequence, acknowledgment, flags, window: 8000, mss: None, payload: &[] };
        tcp::write(&mut bytes, PEER, OURS, &described, data.len());
        bytes
    }

    /// A stack that knows the peer's card, the peer, and a wire.
    fn with_peer() -> (Stack, Sink) {
        let (mut stack, mut wire) = (stack(), Sink::default());
        stack.receive(&arp_frame(arp::REQUEST, PEER_MAC, PEER, [0; 6], OURS), 0, &mut wire);
        wire.0.clear();
        (stack, wire)
    }

    fn from_peer(stack: &mut Stack, wire: &mut Sink, now: u64, bytes: &[u8]) {
        assert_eq!(stack.receive(&ip_frame(PEER_MAC, PEER, OURS, ipv4::PROTOCOL_TCP, bytes), now, wire), Received::Handled);
    }

    /// The segments in what the stack sent, and the wire emptied.
    fn segments(wire: &mut Sink) -> Vec<(u16, u16, u32, u32, u8, Vec<u8>)> {
        let sent: Vec<_> = wire
            .0
            .iter()
            .map(|frame| {
                let (frame, header, payload) = read(frame);
                assert_eq!((frame.to, header.protocol, header.destination), (PEER_MAC, ipv4::PROTOCOL_TCP, PEER));
                let segment = tcp::parse(OURS, PEER, payload).expect("a segment with a good checksum");
                (segment.source_port, segment.destination_port, segment.sequence, segment.acknowledgment, segment.flags, segment.payload.to_vec())
            })
            .collect();
        wire.0.clear();
        sent
    }

    /// A connection from the peer's `port` to port 7 here, made: its
    /// number, and where this side's numbering starts.
    fn accepted(stack: &mut Stack, wire: &mut Sink, port: u16) -> (usize, u32) {
        from_peer(stack, wire, 0, &segment(port, 7, 100, 0, tcp::SYN, &[]));
        let answer = segments(wire);
        assert_eq!(answer.len(), 1);
        let first = answer[0].2;
        assert_eq!(answer[0], (7, port, first, 101, tcp::SYN | tcp::ACK, vec![]));
        assert_eq!(stack.tcp_accept(7), None, "not before the handshake is done");
        from_peer(stack, wire, 0, &segment(port, 7, 101, first.wrapping_add(1), tcp::ACK, &[]));
        let (n, from, from_port) = stack.tcp_accept(7).expect("a connection to take");
        assert_eq!((from, from_port), (PEER, port));
        (n, first)
    }

    #[test]
    fn what_belongs_to_no_connection_is_told_so() {
        let (mut stack, mut wire) = with_peer();
        // A SYN for a port nobody listens on: "no" to all of it.
        from_peer(&mut stack, &mut wire, 0, &segment(4000, 80, 100, 0, tcp::SYN, &[]));
        assert_eq!(segments(&mut wire), [(80, 4000, 0, 101, tcp::RST | tcp::ACK, vec![])]);
        // Data for a connection that is not: "no", with the number its
        // sender expects next.
        from_peer(&mut stack, &mut wire, 0, &segment(4000, 80, 100, 7777, tcp::ACK, b"abc"));
        assert_eq!(segments(&mut wire), [(80, 4000, 7777, 0, tcp::RST, vec![])]);
        assert_eq!((stack.counters().refused, stack.counters().segments_in), (2, 2));
        // A RST is never answered.
        from_peer(&mut stack, &mut wire, 0, &segment(4000, 80, 100, 0, tcp::RST, &[]));
        assert!(wire.0.is_empty());
        // Nor is anything sent to everyone.
        let mut to_all = ip_frame(PEER_MAC, PEER, Ipv4([10, 0, 2, 255]), ipv4::PROTOCOL_TCP, &[0; 20]);
        let mut bytes = vec![0u8; 20];
        tcp::write(
            &mut bytes,
            PEER,
            Ipv4([10, 0, 2, 255]),
            &tcp::Segment { source_port: 1, destination_port: 7, sequence: 1, acknowledgment: 0, flags: tcp::SYN, window: 1, mss: None, payload: &[] },
            0,
        );
        to_all[34..54].copy_from_slice(&bytes);
        assert_eq!(stack.receive(&to_all, 0, &mut wire), Received::Dropped(Why::NotForUs));
        // A segment with a wrong checksum is no segment.
        let mut damaged = segment(4000, 80, 100, 0, tcp::SYN, &[]);
        damaged[5] ^= 1;
        assert_eq!(stack.receive(&ip_frame(PEER_MAC, PEER, OURS, ipv4::PROTOCOL_TCP, &damaged), 0, &mut wire), Received::Dropped(Why::BadChecksum));
        assert!(wire.0.is_empty());
        assert_eq!((stack.counters().refused, stack.counters().segments_in, stack.counters().bad), (2, 3, 1));
    }

    #[test]
    fn a_port_that_is_listened_on_takes_connections() {
        let (mut stack, mut wire) = with_peer();
        assert!(stack.tcp_port_free(7));
        assert!(stack.tcp_listen(7));
        assert!(!stack.tcp_listen(7), "one listener to a port");
        assert!(!stack.tcp_listen(0));
        assert!(!stack.tcp_port_free(7));
        let (n, first) = accepted(&mut stack, &mut wire, 4000);
        assert_eq!(stack.tcp_accept(7), None, "handed out once");
        // Data in, and out.
        from_peer(&mut stack, &mut wire, 5, &segment(4000, 7, 101, first + 1, tcp::ACK | tcp::PSH, b"hello"));
        assert_eq!(segments(&mut wire), [(7, 4000, first + 1, 106, tcp::ACK, vec![])]);
        let mut data = [0u8; 16];
        assert_eq!(stack.tcp_peek(n, &mut data), 5);
        assert_eq!(&data[..5], b"hello");
        assert_eq!(stack.tcp(n).map(|connection| connection.readable()), Some(5), "peeking takes nothing");
        stack.tcp_consume(n, 5, 5, &mut wire);
        assert_eq!(stack.tcp_write(n, b"hello yourself", 6, &mut wire), 14);
        assert_eq!(segments(&mut wire), [(7, 4000, first + 1, 106, tcp::ACK | tcp::PSH, b"hello yourself".to_vec())]);
        // The same ports from another machine: not this connection's.
        // (The gateway says who it is first, so the answer to it waits
        // for no question.)
        stack.receive(&arp_frame(arp::REQUEST, GATEWAY_MAC, GATEWAY, [0; 6], OURS), 6, &mut wire);
        let mut stranger = vec![0u8; 23];
        stranger[20..].copy_from_slice(b"bad");
        let described = tcp::Segment {
            source_port: 4000,
            destination_port: 7,
            sequence: 106,
            acknowledgment: first + 15,
            flags: tcp::ACK,
            window: 8000,
            mss: None,
            payload: &[],
        };
        tcp::write(&mut stranger, GATEWAY, OURS, &described, 3);
        stack.receive(&ip_frame(GATEWAY_MAC, GATEWAY, OURS, ipv4::PROTOCOL_TCP, &stranger), 6, &mut wire);
        assert_eq!(stack.tcp(n).map(|connection| connection.readable()), Some(0));
        assert_eq!(stack.counters().refused, 1);
        wire.0.clear();
        // A SYN+ACK to a port that is listened on opens nothing: it
        // answers a SYN never sent, and is told so.
        from_peer(&mut stack, &mut wire, 6, &segment(4999, 7, 100, 3333, tcp::SYN | tcp::ACK, &[]));
        assert_eq!(segments(&mut wire), [(7, 4999, 3333, 0, tcp::RST, vec![])]);
        assert_eq!(stack.tcp_accept(7), None);
        // A second connection to the same port, from another port of
        // the same machine, is another connection.
        let (m, other) = accepted(&mut stack, &mut wire, 4001);
        assert_ne!(n, m);
        assert_ne!(first, other, "each with a numbering of its own");
        from_peer(&mut stack, &mut wire, 7, &segment(4001, 7, 101, other + 1, tcp::ACK, b"second"));
        assert_eq!((stack.tcp(n).unwrap().readable(), stack.tcp(m).unwrap().readable()), (0, 6));
        wire.0.clear();
        // The program closes the first: FIN, FIN back, and after the
        // wait its place is free.
        from_peer(&mut stack, &mut wire, 8, &segment(4000, 7, 106, first + 15, tcp::ACK, &[]));
        stack.tcp_close(n, 8, &mut wire);
        assert_eq!(segments(&mut wire), [(7, 4000, first + 15, 106, tcp::FIN | tcp::ACK, vec![])]);
        from_peer(&mut stack, &mut wire, 9, &segment(4000, 7, 106, first + 16, tcp::FIN | tcp::ACK, &[]));
        assert_eq!(segments(&mut wire), [(7, 4000, first + 16, 107, tcp::ACK, vec![])]);
        assert_eq!(stack.tcp(n).map(|connection| connection.state()), Some(State::TimeWait));
        assert_eq!(stack.next_timer(), Some(9 + crate::tcp::TIME_WAIT_MS));
        stack.tick(9 + crate::tcp::TIME_WAIT_MS, &mut wire, &mut |_| {});
        assert!(stack.tcp(n).is_none());
        assert!(stack.tcp(m).is_some());
        // Not listened on any more: the one nobody took is cut off,
        // and a new SYN is refused.
        from_peer(&mut stack, &mut wire, 20_000, &segment(4002, 7, 100, 0, tcp::SYN, &[]));
        let answer = segments(&mut wire);
        stack.tcp_unlisten(7, 20_001, &mut wire);
        assert_eq!(segments(&mut wire), [(7, 4002, answer[0].2 + 1, 101, tcp::RST | tcp::ACK, vec![])]);
        from_peer(&mut stack, &mut wire, 20_002, &segment(4003, 7, 100, 0, tcp::SYN, &[]));
        assert_eq!(segments(&mut wire)[0].4, tcp::RST | tcp::ACK);
        assert!(stack.tcp(m).is_some(), "one that was taken is its program's, listener or no listener");
        assert!(!stack.tcp_port_free(7), "and its port is still in use");
        assert_eq!(stack.tcp_local_port(m), Some(7));
        stack.tcp_close(m, 20_003, &mut wire);
        assert_eq!(segments(&mut wire)[0].4, tcp::RST | tcp::ACK, "closed with something unread: cut off");
        assert!(stack.tcp(m).is_none());
        assert!(stack.tcp_port_free(7));
        // Cut off on purpose: the same, whatever was or was not read.
        assert!(stack.tcp_listen(7));
        let (k, first) = accepted(&mut stack, &mut wire, 4004);
        stack.tcp_abort(k, 20_004, &mut wire);
        assert_eq!(segments(&mut wire), [(7, 4004, first + 1, 101, tcp::RST | tcp::ACK, vec![])]);
        assert!(stack.tcp(k).is_none());
    }

    #[test]
    fn only_so_many_connections_wait_for_a_program() {
        let (mut stack, mut wire) = with_peer();
        assert!(stack.tcp_listen(7));
        for port in 0..TCP_BACKLOG as u16 + 2 {
            from_peer(&mut stack, &mut wire, 0, &segment(5000 + port, 7, 100, 0, tcp::SYN, &[]));
        }
        let answers = segments(&mut wire);
        assert_eq!(answers.len(), TCP_BACKLOG, "the rest get no answer, and will ask again");
        assert!(answers.iter().all(|answer| answer.4 == tcp::SYN | tcp::ACK));
        // Half-made connections that are never finished go away by
        // themselves (the SYN+ACK is sent again a few times first).
        let mut again = 0;
        while let Some(now) = stack.next_timer() {
            stack.tick(now, &mut wire, &mut |_| {});
            again += segments(&mut wire).iter().filter(|answer| answer.4 == tcp::SYN | tcp::ACK).count();
        }
        assert_eq!(again, TCP_BACKLOG * usize::from(crate::tcp::SYN_RETRIES));
        assert_eq!(stack.counters().segments_again, again as u64);
        // And then there is room again - for as many connections as
        // there are places, when a program takes them as they come.
        // (By now the peer's card has been forgotten; it speaks up.)
        stack.receive(&arp_frame(arp::REQUEST, PEER_MAC, PEER, [0; 6], OURS), 100_000, &mut wire);
        wire.0.clear();
        let mut taken = Vec::new();
        for port in 0..TCP_CONNECTIONS as u16 + 1 {
            from_peer(&mut stack, &mut wire, 100_000, &segment(6000 + port, 7, 100, 0, tcp::SYN, &[]));
            let Some(answer) = segments(&mut wire).pop() else { break };
            from_peer(&mut stack, &mut wire, 100_000, &segment(6000 + port, 7, 101, answer.2 + 1, tcp::ACK, &[]));
            taken.push(stack.tcp_accept(7).expect("made").0);
        }
        assert_eq!(taken.len(), TCP_CONNECTIONS);
        assert_eq!(stack.tcp_connect(50_000, PEER, 80, 100_000, &mut wire), Err(Sent::NoRoom));
    }

    #[test]
    fn a_connection_out_waits_for_the_card_and_then_for_the_answer() {
        let (mut stack, mut wire) = (stack(), Sink::default());
        stack.seed(0x1234_5678);
        let n = stack.tcp_connect(49152, PEER, 80, 0, &mut wire).unwrap();
        // First, who has that address? The SYN waits.
        assert_eq!(wire.0.len(), 1);
        assert!(is_question(&wire.0[0], PEER));
        wire.0.clear();
        stack.receive(&arp_frame(arp::REPLY, PEER_MAC, PEER, OUR_MAC, OURS), 20, &mut wire);
        let syn = segments(&mut wire);
        assert_eq!(syn.len(), 1);
        let first = syn[0].2;
        assert_eq!(syn[0], (49152, 80, first, 0, tcp::SYN, vec![]));
        assert_eq!(stack.tcp(n).map(|connection| connection.state()), Some(State::SynSent));
        assert!(!stack.tcp_port_free(49152));
        from_peer(&mut stack, &mut wire, 40, &segment(80, 49152, 900, first + 1, tcp::SYN | tcp::ACK, &[]));
        assert_eq!(segments(&mut wire), [(49152, 80, first + 1, 901, tcp::ACK, vec![])]);
        assert_eq!(stack.tcp(n).map(|connection| connection.state()), Some(State::Established));

        // The same ends a little later, or other ends: another first
        // number. And none of them without the secret.
        let numbers = [
            stack.first_number(49152, PEER, 80, 0),
            stack.first_number(49152, PEER, 80, 1000),
            stack.first_number(49153, PEER, 80, 0),
            stack.first_number(49152, GATEWAY, 80, 0),
            stack.first_number(49152, PEER, 81, 0),
        ];
        for (n, number) in numbers.iter().enumerate() {
            assert!(!numbers[..n].contains(number), "{numbers:x?}");
        }
        assert_eq!(numbers[1].wrapping_sub(numbers[0]), 250_000);
        stack.seed(1);
        assert_ne!(stack.first_number(49152, PEER, 80, 0), numbers[0]);

        // Refused: the program can ask why, until it lets go.
        let m = stack.tcp_connect(49153, PEER, 81, 50, &mut wire).unwrap();
        let syn = segments(&mut wire);
        from_peer(&mut stack, &mut wire, 60, &segment(81, 49153, 0, syn[0].2 + 1, tcp::RST | tcp::ACK, &[]));
        assert_eq!(stack.tcp(m).map(|connection| (connection.state(), connection.failure())), Some((State::Closed, Some(crate::tcp::Failure::Refused))));
        stack.tcp_close(m, 60, &mut wire);
        assert!(stack.tcp(m).is_none() && wire.0.is_empty());
    }

    #[test]
    fn a_connection_to_nobody_or_nowhere_fails() {
        let (mut stack, mut wire) = (stack(), Sink::default());
        let nobody = Ipv4([10, 0, 2, 99]);
        let n = stack.tcp_connect(49152, nobody, 80, 0, &mut wire).unwrap();
        let mut told = Vec::new();
        while stack.tcp(n).is_some_and(|connection| connection.state() != State::Closed) {
            let now = stack.next_timer().expect("something is being waited for");
            stack.tick(now, &mut wire, &mut |hop| told.push((hop, now)));
        }
        assert_eq!(stack.tcp(n).and_then(|connection| connection.failure()), Some(crate::tcp::Failure::Unreachable));
        assert_eq!(told, [(nobody, 3000)], "as soon as the question is given up");
        assert!(wire.0.iter().all(|frame| is_question(frame, nobody)), "no SYN ever left");
        // To this machine itself, to port 0, or - without a gateway -
        // to another network: not even tried.
        assert_eq!(stack.tcp_connect(49153, OURS, 80, 0, &mut wire), Err(Sent::NoRoute));
        assert_eq!(stack.tcp_connect(49153, PEER, 0, 0, &mut wire), Err(Sent::NoRoute));
        let mut alone = Stack::new(Config { mac: OUR_MAC, address: OURS, prefix: 24, gateway: Ipv4::NONE });
        assert_eq!(alone.tcp_connect(49153, FAR, 80, 0, &mut wire), Err(Sent::NoRoute));
        // Far away goes by way of the gateway - and fails with it.
        let mut stack = self::stack();
        let far = stack.tcp_connect(49152, FAR, 80, 0, &mut wire).unwrap();
        stack.tick(1000, &mut wire, &mut |_| {});
        stack.tick(2000, &mut wire, &mut |_| {});
        stack.tick(3000, &mut wire, &mut |_| {});
        assert_eq!(stack.tcp(far).and_then(|connection| connection.failure()), Some(crate::tcp::Failure::Unreachable));
    }

    #[test]
    fn a_connection_carries_a_lot_both_ways_through_the_stack() {
        // Two stacks, wired to each other: everything one sends, the
        // other receives.
        let config = |mac, address| Config { mac, address, prefix: 24, gateway: Ipv4::NONE };
        let (mut a, mut b) = (Stack::new(config(OUR_MAC, OURS)), Stack::new(config(PEER_MAC, PEER)));
        assert!(b.tcp_listen(80));
        let (mut to_b, mut to_a) = (Sink::default(), Sink::default());
        let there: Vec<u8> = (0..70_000u32).map(|n| (n * 7 + n / 251) as u8).collect();
        let back: Vec<u8> = (0..30_000u32).map(|n| (n * 13 + n / 127) as u8).collect();
        let n = a.tcp_connect(49152, PEER, 80, 0, &mut to_b).unwrap();
        let (mut m, mut written, mut answered) = (None, 0, 0);
        let (mut got_there, mut got_back) = (Vec::new(), Vec::new());
        let mut piece = [0u8; 999];
        for now in (0..20_000).step_by(10) {
            for frame in core::mem::take(&mut to_b.0) {
                b.receive(&frame, now, &mut to_a);
            }
            for frame in core::mem::take(&mut to_a.0) {
                a.receive(&frame, now, &mut to_b);
            }
            a.tick(now, &mut to_b, &mut |_| {});
            b.tick(now, &mut to_a, &mut |_| {});
            written += a.tcp_write(n, &there[written..], now, &mut to_b);
            if written == there.len() {
                a.tcp_shutdown(n, now, &mut to_b);
            }
            let count = a.tcp_peek(n, &mut piece);
            got_back.extend_from_slice(&piece[..count]);
            a.tcp_consume(n, count, now, &mut to_b);
            m = m.or_else(|| b.tcp_accept(80).map(|(m, _, _)| m));
            if let Some(m) = m {
                let count = b.tcp_peek(m, &mut piece);
                got_there.extend_from_slice(&piece[..count]);
                b.tcp_consume(m, count, now, &mut to_a);
                answered += b.tcp_write(m, &back[answered..], now, &mut to_a);
                if answered == back.len() && b.tcp(m).is_some_and(|connection| connection.finished()) {
                    b.tcp_shutdown(m, now, &mut to_a);
                }
            }
        }
        assert!(got_there == there, "{} of {}", got_there.len(), there.len());
        assert!(got_back == back, "{} of {}", got_back.len(), back.len());
        assert_eq!(a.tcp(n).map(|connection| connection.state()), Some(State::Closed), "after its TimeWait");
        assert_eq!(b.tcp(m.unwrap()).map(|connection| (connection.state(), connection.failure())), Some((State::Closed, None)));
        assert_eq!((a.counters().segments_again, b.counters().segments_again), (0, 0), "nothing was lost: nothing sent twice");
    }
}
