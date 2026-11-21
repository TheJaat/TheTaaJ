//! What one machine on a network does: the stack.
//!
//! It is handed every frame the card received (`receive`) and the time
//! (`tick`), and is asked to send things (`send_udp`, `send_echo`).
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
//!     this wire, to the gateway (a router) if it is not.
//!
//! Time is a number of milliseconds from anywhere, only ever compared.

use crate::wire::{arp, ethernet, icmp, ipv4, udp, Bad};
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
        }
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
        self.neighbours
            .iter()
            .filter_map(|neighbour| match *neighbour {
                Neighbour::Asking { next, .. } => Some(next),
                _ => None,
            })
            .min()
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

        // Protocols nothing here speaks: TCP (6), and IPv6 as a frame type.
        let frame = ip_frame(PEER_MAC, PEER, OURS, 6, &[0; 20]);
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
}
