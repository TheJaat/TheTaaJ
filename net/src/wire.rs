//! Each protocol's header: read from bytes, and written to bytes.
//!
//! Reading never trusts a length the packet states about itself: every
//! one is checked against the bytes that are really there, and a
//! packet that lies is no packet (`None`, or a `Bad` that says how).
//! Numbers on a network are big-endian - the most significant byte
//! first - whatever the machine: `from_be_bytes` / `to_be_bytes`.

use crate::{checksum, Ipv4, Mac};

fn be16(bytes: &[u8], at: usize) -> u16 {
    u16::from_be_bytes([bytes[at], bytes[at + 1]])
}

fn mac(bytes: &[u8], at: usize) -> Mac {
    let mut mac = [0u8; 6];
    mac.copy_from_slice(&bytes[at..at + 6]);
    mac
}

fn ip(bytes: &[u8], at: usize) -> Ipv4 {
    Ipv4([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

/// What was wrong with a packet.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Bad {
    /// Shorter than its own header, or than it says it is.
    Length,
    /// Not the version, or the kind, this code reads.
    Kind,
    /// The checksum does not match the bytes.
    Checksum,
}

/// Ethernet: who a frame is for, who it is from, what is in it.
pub mod ethernet {
    use super::*;

    pub const HEADER: usize = 14;
    pub const TYPE_IPV4: u16 = 0x0800;
    pub const TYPE_ARP: u16 = 0x0806;

    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub struct Frame<'a> {
        pub to: Mac,
        pub from: Mac,
        pub kind: u16,
        pub payload: &'a [u8],
    }

    pub fn parse(frame: &[u8]) -> Option<Frame<'_>> {
        (frame.len() >= HEADER).then(|| Frame { to: mac(frame, 0), from: mac(frame, 6), kind: be16(frame, 12), payload: &frame[HEADER..] })
    }

    /// Writes the header at the start of `frame`.
    pub fn write(frame: &mut [u8], to: Mac, from: Mac, kind: u16) {
        frame[..6].copy_from_slice(&to);
        frame[6..12].copy_from_slice(&from);
        frame[12..14].copy_from_slice(&kind.to_be_bytes());
    }
}

/// ARP (RFC 826): finding the card that has an IPv4 address.
///
/// A question is sent to every card: "who has `target_ip`? Tell
/// `sender_ip`, at `sender_mac`." The one that has it answers, to the
/// asker alone, with itself as the sender.
pub mod arp {
    use super::*;

    pub const SIZE: usize = 28;
    pub const REQUEST: u16 = 1;
    pub const REPLY: u16 = 2;

    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub struct Packet {
        pub operation: u16,
        pub sender_mac: Mac,
        pub sender_ip: Ipv4,
        pub target_mac: Mac,
        pub target_ip: Ipv4,
    }

    /// `payload` is an Ethernet frame's: it may be longer than the
    /// packet (a short frame is filled up).
    pub fn parse(payload: &[u8]) -> Option<Packet> {
        if payload.len() < SIZE {
            return None;
        }
        // Card addresses of Ethernet (1), six bytes; machine addresses
        // of IPv4, four. ARP could carry others; nothing here does.
        if be16(payload, 0) != 1 || be16(payload, 2) != ethernet::TYPE_IPV4 || payload[4] != 6 || payload[5] != 4 {
            return None;
        }
        Some(Packet {
            operation: be16(payload, 6),
            sender_mac: mac(payload, 8),
            sender_ip: ip(payload, 14),
            target_mac: mac(payload, 18),
            target_ip: ip(payload, 24),
        })
    }

    impl Packet {
        pub fn write(&self, out: &mut [u8]) {
            out[0..2].copy_from_slice(&1u16.to_be_bytes());
            out[2..4].copy_from_slice(&ethernet::TYPE_IPV4.to_be_bytes());
            out[4] = 6;
            out[5] = 4;
            out[6..8].copy_from_slice(&self.operation.to_be_bytes());
            out[8..14].copy_from_slice(&self.sender_mac);
            out[14..18].copy_from_slice(&self.sender_ip.0);
            out[18..24].copy_from_slice(&self.target_mac);
            out[24..28].copy_from_slice(&self.target_ip.0);
        }
    }
}

/// IPv4 (RFC 791): a packet from one machine to another.
pub mod ipv4 {
    use super::*;

    /// The header this code writes: no options.
    pub const HEADER: usize = 20;
    pub const PROTOCOL_ICMP: u8 = 1;
    pub const PROTOCOL_TCP: u8 = 6;
    pub const PROTOCOL_UDP: u8 = 17;
    /// How many routers a packet sent from here may pass.
    pub const TTL: u8 = 64;

    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub struct Header {
        /// In bytes: 20, or more with options.
        pub header_length: usize,
        pub identification: u16,
        /// A piece of a packet that was cut up on the way: more pieces
        /// follow, or this one does not begin at the beginning.
        pub fragment: bool,
        pub ttl: u8,
        pub protocol: u8,
        pub source: Ipv4,
        pub destination: Ipv4,
    }

    /// The header and the payload of `packet` - an Ethernet frame's
    /// payload, which may be longer than the packet says it is (short
    /// frames are filled up); what is beyond is not the packet's.
    pub fn parse(packet: &[u8]) -> Result<(Header, &[u8]), Bad> {
        if packet.len() < HEADER {
            return Err(Bad::Length);
        }
        if packet[0] >> 4 != 4 {
            return Err(Bad::Kind);
        }
        let header_length = usize::from(packet[0] & 0xF) * 4;
        let total = usize::from(be16(packet, 2));
        if header_length < HEADER || total < header_length || total > packet.len() {
            return Err(Bad::Length);
        }
        if checksum::of(&packet[..header_length]) != 0 {
            return Err(Bad::Checksum);
        }
        let flags_and_offset = be16(packet, 6);
        let header = Header {
            header_length,
            identification: be16(packet, 4),
            // Bit 13 is "more fragments"; the low 13 bits are where
            // this piece belongs.
            fragment: flags_and_offset & 0x3FFF != 0,
            ttl: packet[8],
            protocol: packet[9],
            source: ip(packet, 12),
            destination: ip(packet, 16),
        };
        Ok((header, &packet[header_length..total]))
    }

    /// Writes a header at the start of `packet`, for a payload of
    /// `payload` bytes after it.
    pub fn write(packet: &mut [u8], protocol: u8, source: Ipv4, destination: Ipv4, identification: u16, payload: usize) {
        packet[0] = 0x45; // version 4, a header of 5 words
        packet[1] = 0;
        packet[2..4].copy_from_slice(&((HEADER + payload) as u16).to_be_bytes());
        packet[4..6].copy_from_slice(&identification.to_be_bytes());
        // "Do not fragment": nothing here could put the pieces together.
        packet[6..8].copy_from_slice(&0x4000u16.to_be_bytes());
        packet[8] = TTL;
        packet[9] = protocol;
        packet[10..12].copy_from_slice(&[0, 0]);
        packet[12..16].copy_from_slice(&source.0);
        packet[16..20].copy_from_slice(&destination.0);
        let sum = checksum::of(&packet[..HEADER]);
        packet[10..12].copy_from_slice(&sum.to_be_bytes());
    }
}

/// ICMP (RFC 792): the network's own messages.
pub mod icmp {
    use super::*;

    pub const HEADER: usize = 8;
    pub const ECHO_REPLY: u8 = 0;
    pub const UNREACHABLE: u8 = 3;
    /// With UNREACHABLE: the machine is there, no program has the port.
    pub const CODE_PORT: u8 = 3;
    pub const ECHO_REQUEST: u8 = 8;

    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub struct Message<'a> {
        pub kind: u8,
        pub code: u8,
        /// Four bytes whose meaning depends on the kind: for an echo,
        /// an identifier and a sequence number.
        pub rest: [u8; 4],
        pub data: &'a [u8],
    }

    impl Message<'_> {
        pub fn identifier(&self) -> u16 {
            u16::from_be_bytes([self.rest[0], self.rest[1]])
        }

        pub fn sequence(&self) -> u16 {
            u16::from_be_bytes([self.rest[2], self.rest[3]])
        }
    }

    pub fn parse(message: &[u8]) -> Result<Message<'_>, Bad> {
        if message.len() < HEADER {
            return Err(Bad::Length);
        }
        if checksum::of(message) != 0 {
            return Err(Bad::Checksum);
        }
        Ok(Message { kind: message[0], code: message[1], rest: [message[4], message[5], message[6], message[7]], data: &message[HEADER..] })
    }

    /// Writes the header at the start of `message`, whose data - `data`
    /// bytes after the header - is already there. Returns the length.
    pub fn write(message: &mut [u8], kind: u8, code: u8, rest: [u8; 4], data: usize) -> usize {
        message[0] = kind;
        message[1] = code;
        message[2..4].copy_from_slice(&[0, 0]);
        message[4..8].copy_from_slice(&rest);
        let sum = checksum::of(&message[..HEADER + data]);
        message[2..4].copy_from_slice(&sum.to_be_bytes());
        HEADER + data
    }
}

/// UDP (RFC 768): a datagram from a port on one machine to a port on
/// another. Nothing more: no order, no promise that it arrives.
pub mod udp {
    use super::*;

    pub const HEADER: usize = 8;

    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub struct Datagram<'a> {
        pub source_port: u16,
        pub destination_port: u16,
        pub payload: &'a [u8],
    }

    /// The sum of the "pseudo-header": UDP's checksum also covers the
    /// two addresses and the length, which are not in its own header -
    /// so a datagram delivered to the wrong machine fails it.
    fn pseudo(source: Ipv4, destination: Ipv4, length: usize) -> u32 {
        let sum = checksum::add(0, &source.0);
        let sum = checksum::add(sum, &destination.0);
        sum + u32::from(ipv4::PROTOCOL_UDP) + length as u32
    }

    /// `segment` is an IPv4 packet's payload; the addresses are that
    /// packet's.
    pub fn parse(source: Ipv4, destination: Ipv4, segment: &[u8]) -> Result<Datagram<'_>, Bad> {
        if segment.len() < HEADER {
            return Err(Bad::Length);
        }
        let length = usize::from(be16(segment, 4));
        if length < HEADER || length > segment.len() {
            return Err(Bad::Length);
        }
        let segment = &segment[..length];
        // A checksum of zero means the sender made none (allowed, over
        // IPv4).
        if be16(segment, 6) != 0 && checksum::finish(checksum::add(pseudo(source, destination, length), segment)) != 0 {
            return Err(Bad::Checksum);
        }
        Ok(Datagram { source_port: be16(segment, 0), destination_port: be16(segment, 2), payload: &segment[HEADER..] })
    }

    /// Writes the header at the start of `segment`, whose payload -
    /// `payload` bytes after the header - is already there. Returns
    /// the length.
    pub fn write(segment: &mut [u8], source: Ipv4, destination: Ipv4, source_port: u16, destination_port: u16, payload: usize) -> usize {
        let length = HEADER + payload;
        segment[0..2].copy_from_slice(&source_port.to_be_bytes());
        segment[2..4].copy_from_slice(&destination_port.to_be_bytes());
        segment[4..6].copy_from_slice(&(length as u16).to_be_bytes());
        segment[6..8].copy_from_slice(&[0, 0]);
        let sum = checksum::finish(checksum::add(pseudo(source, destination, length), &segment[..length]));
        // Zero is taken: it means "no checksum". A sum that comes out
        // zero is sent as 0xFFFF, which adds up to the same.
        let sum = if sum == 0 { 0xFFFF } else { sum };
        segment[6..8].copy_from_slice(&sum.to_be_bytes());
        length
    }
}

/// TCP (RFC 9293): a segment of a connection - a stream of bytes in
/// each direction between a port on one machine and a port on another,
/// every byte numbered.
pub mod tcp {
    use super::*;

    /// The header without options.
    pub const HEADER: usize = 20;
    /// The longest header: the length is counted in 4-byte words, in 4
    /// bits.
    pub const HEADER_MAX: usize = 60;

    /// The sender has no more to send.
    pub const FIN: u8 = 0x01;
    /// The first segment of a connection: "my numbering starts here".
    pub const SYN: u8 = 0x02;
    /// There is no such connection: forget it.
    pub const RST: u8 = 0x04;
    /// Hand this to the program now, do not wait for more.
    pub const PSH: u8 = 0x08;
    /// The acknowledgment field means something.
    pub const ACK: u8 = 0x10;

    /// The one option read here: the longest segment the sender is
    /// willing to receive. Kind 2, length 4; only in a SYN.
    const OPTION_END: u8 = 0;
    const OPTION_NOTHING: u8 = 1;
    const OPTION_MSS: u8 = 2;

    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub struct Segment<'a> {
        pub source_port: u16,
        pub destination_port: u16,
        /// The number of this segment's first byte.
        pub sequence: u32,
        /// The number of the next byte the sender expects (if ACK).
        pub acknowledgment: u32,
        pub flags: u8,
        /// How many more bytes the sender has room for.
        pub window: u16,
        /// From a "maximum segment size" option, if there was one.
        pub mss: Option<u16>,
        pub payload: &'a [u8],
    }

    impl Segment<'_> {
        pub fn has(&self, flag: u8) -> bool {
            self.flags & flag != 0
        }

        /// How many numbers the segment takes up: one for each byte,
        /// and one each for SYN and FIN - which is what lets those two
        /// be acknowledged, and sent again if they are lost.
        pub fn length(&self) -> u32 {
            self.payload.len() as u32 + u32::from(self.has(SYN)) + u32::from(self.has(FIN))
        }
    }

    /// As UDP's: the two addresses, the protocol and the length.
    fn pseudo(source: Ipv4, destination: Ipv4, length: usize) -> u32 {
        let sum = checksum::add(0, &source.0);
        let sum = checksum::add(sum, &destination.0);
        sum + u32::from(ipv4::PROTOCOL_TCP) + length as u32
    }

    /// `segment` is an IPv4 packet's payload; the addresses are that
    /// packet's. (Unlike UDP there is no length field: a segment is as
    /// long as the packet says its payload is.)
    pub fn parse(source: Ipv4, destination: Ipv4, segment: &[u8]) -> Result<Segment<'_>, Bad> {
        if segment.len() < HEADER {
            return Err(Bad::Length);
        }
        let header = usize::from(segment[12] >> 4) * 4;
        if header < HEADER || header > segment.len() {
            return Err(Bad::Length);
        }
        // The checksum is not optional here.
        if checksum::finish(checksum::add(pseudo(source, destination, segment.len()), segment)) != 0 {
            return Err(Bad::Checksum);
        }
        // The options: a kind, a length (which counts both), the rest.
        // Two kinds are a single byte. One that does not fit ends the
        // reading; the segment is still good.
        let mut mss = None;
        let mut options = &segment[HEADER..header];
        while let [kind, rest @ ..] = options {
            match *kind {
                OPTION_END => break,
                OPTION_NOTHING => options = rest,
                kind => {
                    let Some(&length) = rest.first() else { break };
                    let length = usize::from(length);
                    if length < 2 || length > options.len() {
                        break;
                    }
                    if kind == OPTION_MSS && length == 4 {
                        mss = Some(be16(options, 2));
                    }
                    options = &options[length..];
                }
            }
        }
        Ok(Segment {
            source_port: be16(segment, 0),
            destination_port: be16(segment, 2),
            sequence: u32::from_be_bytes([segment[4], segment[5], segment[6], segment[7]]),
            acknowledgment: u32::from_be_bytes([segment[8], segment[9], segment[10], segment[11]]),
            flags: segment[13] & 0x3F,
            window: be16(segment, 14),
            mss,
            payload: &segment[header..],
        })
    }

    /// How long the header of `segment` will be: 24 bytes with the one
    /// option, 20 without.
    pub fn header_length(mss: Option<u16>) -> usize {
        if mss.is_some() {
            HEADER + 4
        } else {
            HEADER
        }
    }

    /// Writes the header of `described` at the start of `segment`; its
    /// payload - `payload` bytes after the header (see
    /// `header_length`) - is already there. `described.payload` is not
    /// looked at. Returns the length of the whole.
    pub fn write(segment: &mut [u8], source: Ipv4, destination: Ipv4, described: &Segment<'_>, payload: usize) -> usize {
        let header = header_length(described.mss);
        let length = header + payload;
        segment[0..2].copy_from_slice(&described.source_port.to_be_bytes());
        segment[2..4].copy_from_slice(&described.destination_port.to_be_bytes());
        segment[4..8].copy_from_slice(&described.sequence.to_be_bytes());
        segment[8..12].copy_from_slice(&described.acknowledgment.to_be_bytes());
        segment[12] = ((header / 4) as u8) << 4;
        segment[13] = described.flags;
        segment[14..16].copy_from_slice(&described.window.to_be_bytes());
        segment[16..20].copy_from_slice(&[0; 4]); // the checksum, and "urgent": not used
        if let Some(mss) = described.mss {
            segment[20] = OPTION_MSS;
            segment[21] = 4;
            segment[22..24].copy_from_slice(&mss.to_be_bytes());
        }
        let sum = checksum::finish(checksum::add(pseudo(source, destination, length), &segment[..length]));
        segment[16..18].copy_from_slice(&sum.to_be_bytes());
        length
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A ping as `tcpdump -x` shows one: an Ethernet frame from
    // 52:54:00:12:34:56 to 52:55:0a:00:02:02, carrying an IPv4 packet
    // from 10.0.2.15 to 10.0.2.2, carrying an ICMP echo request with
    // identifier 0x1234, sequence 1 and the data "abcdefgh". Written
    // out by hand, its two checksums worked out apart from this code
    // (a few lines of Python), so that the code is checked against the
    // standards and not against itself.
    const PING: [u8; 50] = [
        0x52, 0x55, 0x0a, 0x00, 0x02, 0x02, 0x52, 0x54, 0x00, 0x12, 0x34, 0x56, 0x08, 0x00, // Ethernet
        0x45, 0x00, 0x00, 0x24, 0x00, 0x01, 0x40, 0x00, 0x40, 0x01, 0x22, 0xc8, // IPv4 ...
        0x0a, 0x00, 0x02, 0x0f, 0x0a, 0x00, 0x02, 0x02, // ... its addresses
        0x08, 0x00, 0x54, 0x35, 0x12, 0x34, 0x00, 0x01, // ICMP
        b'a', b'b', b'c', b'd', b'e', b'f', b'g', b'h',
    ];

    #[test]
    fn a_ping_is_read_layer_by_layer() {
        let frame = ethernet::parse(&PING).unwrap();
        assert_eq!(frame.to, [0x52, 0x55, 0x0a, 0x00, 0x02, 0x02]);
        assert_eq!(frame.from, [0x52, 0x54, 0x00, 0x12, 0x34, 0x56]);
        assert_eq!(frame.kind, ethernet::TYPE_IPV4);
        let (header, payload) = ipv4::parse(frame.payload).unwrap();
        assert_eq!(header.source, Ipv4([10, 0, 2, 15]));
        assert_eq!(header.destination, Ipv4([10, 0, 2, 2]));
        assert_eq!((header.protocol, header.ttl, header.header_length, header.fragment), (ipv4::PROTOCOL_ICMP, 64, 20, false));
        let message = icmp::parse(payload).unwrap();
        assert_eq!((message.kind, message.code), (icmp::ECHO_REQUEST, 0));
        assert_eq!((message.identifier(), message.sequence()), (0x1234, 1));
        assert_eq!(message.data, b"abcdefgh");
    }

    #[test]
    fn a_ping_is_written_byte_for_byte() {
        let mut frame = [0u8; 50];
        frame[42..].copy_from_slice(b"abcdefgh");
        let length = icmp::write(&mut frame[34..], icmp::ECHO_REQUEST, 0, [0x12, 0x34, 0x00, 0x01], 8);
        ipv4::write(&mut frame[14..], ipv4::PROTOCOL_ICMP, Ipv4([10, 0, 2, 15]), Ipv4([10, 0, 2, 2]), 1, length);
        ethernet::write(&mut frame, [0x52, 0x55, 0x0a, 0x00, 0x02, 0x02], [0x52, 0x54, 0x00, 0x12, 0x34, 0x56], ethernet::TYPE_IPV4);
        assert_eq!(frame, PING);
    }

    #[test]
    fn a_frame_too_short_is_no_frame() {
        assert!(ethernet::parse(&PING[..13]).is_none());
        assert_eq!(ethernet::parse(&PING[..14]).unwrap().payload, &[] as &[u8]);
    }

    #[test]
    fn arp_there_and_back() {
        let question = arp::Packet {
            operation: arp::REQUEST,
            sender_mac: [2, 0, 0, 0, 0, 1],
            sender_ip: Ipv4([10, 0, 2, 2]),
            target_mac: [0; 6],
            target_ip: Ipv4([10, 0, 2, 15]),
        };
        let mut bytes = [0u8; 46]; // as in a frame: filled up
        question.write(&mut bytes);
        // "Who has 10.0.2.15? Tell 10.0.2.2", by the book.
        assert_eq!(bytes[..arp::SIZE], [0, 1, 8, 0, 6, 4, 0, 1, 2, 0, 0, 0, 0, 1, 10, 0, 2, 2, 0, 0, 0, 0, 0, 0, 10, 0, 2, 15]);
        assert_eq!(arp::parse(&bytes), Some(question));
        assert_eq!(arp::parse(&bytes[..27]), None);
        // Not Ethernet, not IPv4, wrong address lengths: not read.
        for (at, value) in [(1, 6), (3, 0xDD), (4, 8), (5, 16)] {
            let mut wrong = bytes;
            wrong[at] = value;
            assert_eq!(arp::parse(&wrong), None, "byte {at}");
        }
    }

    fn packet(payload: &[u8]) -> Vec<u8> {
        let mut packet = vec![0u8; ipv4::HEADER + payload.len()];
        packet[ipv4::HEADER..].copy_from_slice(payload);
        ipv4::write(&mut packet, ipv4::PROTOCOL_UDP, Ipv4([10, 0, 2, 2]), Ipv4([10, 0, 2, 15]), 7, payload.len());
        packet
    }

    /// Sets bytes of a header and makes its checksum right again.
    fn patched(mut packet: Vec<u8>, changes: &[(usize, u8)]) -> Vec<u8> {
        for &(at, value) in changes {
            packet[at] = value;
        }
        let header = usize::from(packet[0] & 0xF) * 4;
        if header >= 20 && header <= packet.len() {
            packet[10..12].copy_from_slice(&[0, 0]);
            let sum = checksum::of(&packet[..header]);
            packet[10..12].copy_from_slice(&sum.to_be_bytes());
        }
        packet
    }

    #[test]
    fn an_ipv4_packet_must_be_what_it_says() {
        let good = packet(b"hello");
        let (header, payload) = ipv4::parse(&good).unwrap();
        assert_eq!(payload, b"hello");
        assert_eq!((header.identification, header.protocol), (7, ipv4::PROTOCOL_UDP));

        // In a frame that was filled up: what is beyond is not its.
        let mut padded = good.clone();
        padded.extend_from_slice(&[0; 21]);
        assert_eq!(ipv4::parse(&padded).unwrap().1, b"hello");

        assert_eq!(ipv4::parse(&good[..19]), Err(Bad::Length));
        assert_eq!(ipv4::parse(&good[..24]), Err(Bad::Length), "says 25 bytes, has 24");
        assert_eq!(ipv4::parse(&patched(good.clone(), &[(0, 0x65)])), Err(Bad::Kind), "version 6");
        assert_eq!(ipv4::parse(&patched(good.clone(), &[(0, 0x44)])), Err(Bad::Length), "a header of 16 bytes");
        assert_eq!(ipv4::parse(&patched(good.clone(), &[(2, 0), (3, 19)])), Err(Bad::Length), "shorter than its header");
        assert_eq!(ipv4::parse(&patched(good.clone(), &[(0, 0x4F)])), Err(Bad::Length), "a header longer than the packet");
        // One bit anywhere in the header.
        for at in 0..ipv4::HEADER {
            let mut wrong = good.clone();
            wrong[at] ^= 0x10;
            assert!(ipv4::parse(&wrong).is_err(), "byte {at}");
        }
        // The payload is not the header's to check.
        let mut other = good.clone();
        other[22] ^= 0xFF;
        assert!(ipv4::parse(&other).is_ok());
    }

    #[test]
    fn options_are_stepped_over_and_fragments_are_named() {
        // A header of 24 bytes: four bytes of options ("no operation").
        let mut long = vec![0u8; 24 + 3];
        long[..20].copy_from_slice(&packet(b"")[..20]);
        long[20..24].copy_from_slice(&[1, 1, 1, 1]);
        long[24..].copy_from_slice(b"abc");
        let long = patched(long, &[(0, 0x46), (3, 27)]);
        let (header, payload) = ipv4::parse(&long).unwrap();
        assert_eq!((header.header_length, payload), (24, &b"abc"[..]));

        let whole = packet(b"x");
        assert!(!ipv4::parse(&whole).unwrap().0.fragment, "'do not fragment' is not a fragment");
        assert!(ipv4::parse(&patched(whole.clone(), &[(6, 0x20)])).unwrap().0.fragment, "more follow");
        assert!(ipv4::parse(&patched(whole.clone(), &[(6, 0), (7, 5)])).unwrap().0.fragment, "not the beginning");
    }

    #[test]
    fn icmp_is_checked_whole() {
        let mut message = [0u8; 8 + 5];
        message[8..].copy_from_slice(b"12345"); // an odd length
        let length = icmp::write(&mut message, icmp::ECHO_REPLY, 0, [0, 9, 0, 3], 5);
        let read = icmp::parse(&message[..length]).unwrap();
        assert_eq!((read.kind, read.identifier(), read.sequence(), read.data), (icmp::ECHO_REPLY, 9, 3, &b"12345"[..]));
        for at in 0..length {
            let mut wrong = message;
            wrong[at] ^= 1;
            assert_eq!(icmp::parse(&wrong[..length]), Err(Bad::Checksum), "byte {at}");
        }
        assert_eq!(icmp::parse(&message[..7]), Err(Bad::Length));
    }

    #[test]
    fn udp_is_checked_with_its_addresses() {
        let (from, to) = (Ipv4([10, 0, 2, 2]), Ipv4([10, 0, 2, 15]));
        let mut segment = [0u8; 8 + 5 + 3];
        segment[8..13].copy_from_slice(b"hello");
        let length = udp::write(&mut segment, from, to, 5000, 7, 5);
        assert_eq!(length, 13);
        // By hand: ports 5000 and 7, length 13.
        assert_eq!(segment[..6], [0x13, 0x88, 0x00, 0x07, 0x00, 0x0d]);
        // The segment may be followed by a frame's filling.
        let read = udp::parse(from, to, &segment).unwrap();
        assert_eq!((read.source_port, read.destination_port, read.payload), (5000, 7, &b"hello"[..]));

        // Any bit changed - and either address.
        for at in 0..length {
            let mut wrong = segment;
            wrong[at] ^= 0x40;
            assert!(udp::parse(from, to, &wrong).is_err(), "byte {at}");
        }
        assert_eq!(udp::parse(from, Ipv4([10, 0, 2, 16]), &segment), Err(Bad::Checksum));
        assert_eq!(udp::parse(Ipv4([10, 0, 2, 3]), to, &segment), Err(Bad::Checksum));

        // No checksum at all is allowed.
        let mut unchecked = segment;
        unchecked[6..8].copy_from_slice(&[0, 0]);
        assert_eq!(udp::parse(from, Ipv4([1, 2, 3, 4]), &unchecked).unwrap().payload, b"hello");

        // Lengths that lie.
        let mut long = segment;
        long[5] = 17;
        assert_eq!(udp::parse(from, to, &long), Err(Bad::Length));
        let mut short = segment;
        short[5] = 7;
        assert_eq!(udp::parse(from, to, &short), Err(Bad::Length));
        assert_eq!(udp::parse(from, to, &segment[..7]), Err(Bad::Length));
    }

    #[test]
    fn a_udp_checksum_is_never_sent_as_zero() {
        // Search for a payload whose sum comes out zero: it must go out
        // as 0xFFFF, and be accepted.
        let (from, to) = (Ipv4([0, 0, 0, 0]), Ipv4([0, 0, 0, 0]));
        let mut found = false;
        for value in 0..=0xFFFFu16 {
            let mut segment = [0u8; 10];
            segment[8..].copy_from_slice(&value.to_be_bytes());
            udp::write(&mut segment, from, to, 0, 0, 2);
            assert_ne!(segment[6..8], [0, 0]);
            assert!(udp::parse(from, to, &segment).is_ok());
            found |= segment[6..8] == [0xFF, 0xFF];
        }
        assert!(found, "no payload reached the case");
    }

    // Three TCP segments from 10.0.2.15 to 10.0.2.2, made by a few
    // lines of Python that know nothing of this code: a SYN with the
    // option "my longest segment is 1460", a segment carrying "hello",
    // and a SYN+ACK whose options are padding, "536", one this code
    // does not read (window scale), the end mark and two bytes to fill
    // the last word.
    const SYN_SEGMENT: [u8; 24] = [
        0xc0, 0x00, 0x00, 0x50, 0x01, 0x02, 0x03, 0x04, 0x00, 0x00, 0x00, 0x00, 0x60, 0x02, 0x40, 0x00, 0x7b, 0xbf, 0x00, 0x00, 0x02, 0x04, 0x05,
        0xb4,
    ];
    const HELLO_SEGMENT: [u8; 25] = [
        0xc0, 0x00, 0x00, 0x50, 0xff, 0xff, 0xff, 0xfe, 0x0a, 0x0b, 0x0c, 0x0d, 0x50, 0x18, 0x10, 0x00, 0x6d, 0x7d, 0x00, 0x00, 0x68, 0x65, 0x6c,
        0x6c, 0x6f,
    ];
    const OPTIONS_SEGMENT: [u8; 32] = [
        0x00, 0x50, 0xc0, 0x00, 0x00, 0x00, 0x00, 0x07, 0x00, 0x00, 0x00, 0x09, 0x80, 0x12, 0xff, 0xff, 0x98, 0x35, 0x00, 0x00, 0x01, 0x01, 0x02,
        0x04, 0x02, 0x18, 0x03, 0x03, 0x07, 0x00, 0x00, 0x00,
    ];
    const FROM: Ipv4 = Ipv4([10, 0, 2, 15]);
    const TO: Ipv4 = Ipv4([10, 0, 2, 2]);

    #[test]
    fn tcp_segments_are_read_as_the_standard_writes_them() {
        let syn = tcp::parse(FROM, TO, &SYN_SEGMENT).unwrap();
        assert_eq!(
            syn,
            tcp::Segment {
                source_port: 49152,
                destination_port: 80,
                sequence: 0x0102_0304,
                acknowledgment: 0,
                flags: tcp::SYN,
                window: 16384,
                mss: Some(1460),
                payload: &[]
            }
        );
        assert_eq!(syn.length(), 1); // the SYN itself takes a number
        let hello = tcp::parse(FROM, TO, &HELLO_SEGMENT).unwrap();
        assert_eq!((hello.sequence, hello.acknowledgment), (0xFFFF_FFFE, 0x0A0B_0C0D));
        assert_eq!((hello.flags, hello.window, hello.mss, hello.payload), (tcp::PSH | tcp::ACK, 4096, None, &b"hello"[..]));
        assert_eq!(hello.length(), 5);
        let options = tcp::parse(FROM, TO, &OPTIONS_SEGMENT).unwrap();
        assert_eq!((options.flags, options.mss, options.payload.len()), (tcp::SYN | tcp::ACK, Some(536), 0));
    }

    #[test]
    fn tcp_segments_are_written_as_the_standard_reads_them() {
        let mut out = [0xEEu8; 64];
        let syn = tcp::parse(FROM, TO, &SYN_SEGMENT).unwrap();
        assert_eq!(tcp::header_length(syn.mss), 24);
        assert_eq!(tcp::write(&mut out, FROM, TO, &syn, 0), 24);
        assert_eq!(out[..24], SYN_SEGMENT);
        let hello = tcp::parse(FROM, TO, &HELLO_SEGMENT).unwrap();
        out[20..25].copy_from_slice(b"hello");
        assert_eq!(tcp::write(&mut out, FROM, TO, &hello, 5), 25);
        assert_eq!(out[..25], HELLO_SEGMENT);
    }

    #[test]
    fn a_tcp_segment_must_be_what_it_says() {
        // Any one byte changed: the checksum says so.
        for at in 0..HELLO_SEGMENT.len() {
            let mut damaged = HELLO_SEGMENT;
            damaged[at] ^= 0x10;
            assert!(tcp::parse(FROM, TO, &damaged).is_err(), "byte {at}");
        }
        // Delivered to another machine: the same.
        assert_eq!(tcp::parse(FROM, Ipv4([10, 0, 2, 3]), &HELLO_SEGMENT), Err(Bad::Checksum));
        // Too short for a header; a header longer than the segment;
        // a header shorter than a header.
        assert_eq!(tcp::parse(FROM, TO, &HELLO_SEGMENT[..19]), Err(Bad::Length));
        let mut long = SYN_SEGMENT;
        long[12] = 0x70;
        assert_eq!(tcp::parse(FROM, TO, &long), Err(Bad::Length));
        let mut short = SYN_SEGMENT;
        short[12] = 0x40;
        assert_eq!(tcp::parse(FROM, TO, &short), Err(Bad::Length));
    }

    #[test]
    fn tcp_options_that_lie_end_the_reading_and_nothing_else() {
        // An option that says it is longer than what is left, one of
        // length 0 (which would never be stepped past), one cut off
        // after its kind: the segment is read, without them.
        for options in [[9u8, 40, 0, 0], [9, 0, 2, 4], [1, 1, 1, 2]] {
            let mut segment = SYN_SEGMENT;
            segment[20..24].copy_from_slice(&options);
            segment[16..18].copy_from_slice(&[0, 0]);
            let sum = checksum::finish(checksum::add(checksum::add(checksum::add(0, &FROM.0), &TO.0) + 6 + 24, &segment));
            segment[16..18].copy_from_slice(&sum.to_be_bytes());
            let read = tcp::parse(FROM, TO, &segment).unwrap();
            assert_eq!((read.mss, read.flags, read.sequence), (None, tcp::SYN, 0x0102_0304), "{options:?}");
        }
    }
}
