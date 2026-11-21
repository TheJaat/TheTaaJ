//! The network protocols, as arithmetic on bytes.
//!
//! Nothing here touches a card, a clock or a channel. `wire` reads and
//! writes the headers of each protocol; `stack` is what a machine on a
//! network does with them - answers "who has this address?", answers
//! pings, finds out which card a packet must go to. It is handed
//! frames and the time, and hands back frames to send and what
//! arrived for the programs above. So all of it runs in a host test,
//! and the service that owns the card (`user/src/bin/net.rs`) is only
//! the plumbing around it.
//!
//! The layers, outermost first - each is the payload of the one before:
//!
//!   Ethernet   from one card to another on the same wire
//!     ARP        "who has address 10.0.2.2?" - "I, card 52:55:..."
//!     IPv4       from one machine to another, across networks
//!       ICMP       the network's own messages: ping, "nobody there"
//!       UDP        from one program to another: a port number each

#![cfg_attr(not(test), no_std)]

pub mod checksum;
pub mod stack;

pub mod wire;

/// A card's address.
pub type Mac = [u8; 6];

/// The card address that means every card on the wire.
pub const EVERYONE: Mac = [0xFF; 6];

/// A machine's address: four bytes, written "10.0.2.15".
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Ipv4(pub [u8; 4]);

impl Ipv4 {
    /// Not an address: "I have none yet", or "any".
    pub const NONE: Ipv4 = Ipv4([0; 4]);
    /// Every machine on this wire.
    pub const BROADCAST: Ipv4 = Ipv4([255; 4]);

    /// From text: four numbers, 0 to 255, with dots between.
    pub fn parse(text: &str) -> Option<Ipv4> {
        let mut bytes = [0u8; 4];
        let mut parts = text.split('.');
        for byte in &mut bytes {
            let part = parts.next()?;
            // (`u8::from_str` would take "+1": only digits are an address.)
            if part.is_empty() || part.len() > 3 || !part.bytes().all(|c| c.is_ascii_digit()) {
                return None;
            }
            *byte = part.parse().ok()?;
        }
        parts.next().is_none().then_some(Ipv4(bytes))
    }

    pub fn to_u32(self) -> u32 {
        u32::from_be_bytes(self.0)
    }

    pub fn from_u32(value: u32) -> Ipv4 {
        Ipv4(value.to_be_bytes())
    }

    /// The bits that name the network, for a prefix of `prefix` bits:
    /// 24 gives 255.255.255.0.
    pub fn mask(prefix: u8) -> u32 {
        match prefix {
            0 => 0,
            1..=31 => !0u32 << (32 - prefix),
            _ => !0,
        }
    }

    /// Whether `other` is on the same network as this address - can be
    /// reached on the wire itself, without a router.
    pub fn same_network(self, other: Ipv4, prefix: u8) -> bool {
        (self.to_u32() ^ other.to_u32()) & Ipv4::mask(prefix) == 0
    }

    /// The address that means every machine on this address's network.
    pub fn network_broadcast(self, prefix: u8) -> Ipv4 {
        Ipv4::from_u32(self.to_u32() | !Ipv4::mask(prefix))
    }
}

impl core::fmt::Display for Ipv4 {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        let [a, b, c, d] = self.0;
        write!(f, "{a}.{b}.{c}.{d}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_address_is_four_numbers_with_dots() {
        assert_eq!(Ipv4::parse("10.0.2.15"), Some(Ipv4([10, 0, 2, 15])));
        assert_eq!(Ipv4::parse("255.255.255.255"), Some(Ipv4::BROADCAST));
        assert_eq!(Ipv4::parse("0.0.0.0"), Some(Ipv4::NONE));
        for wrong in ["", "10.0.2", "10.0.2.15.1", "10.0.2.256", "10..2.15", "10.0.2.+5", "a.b.c.d", "10.0.2.15 ", "1000.0.0.1", ".1.2.3"] {
            assert_eq!(Ipv4::parse(wrong), None, "{wrong:?}");
        }
        assert_eq!(format!("{}", Ipv4([192, 168, 1, 20])), "192.168.1.20");
    }

    #[test]
    fn a_prefix_says_which_addresses_are_neighbours() {
        let ours = Ipv4([10, 0, 2, 15]);
        assert!(ours.same_network(Ipv4([10, 0, 2, 2]), 24));
        assert!(!ours.same_network(Ipv4([10, 0, 3, 2]), 24));
        assert!(ours.same_network(Ipv4([10, 0, 3, 2]), 16));
        assert!(ours.same_network(Ipv4([8, 8, 8, 8]), 0));
        assert!(!ours.same_network(Ipv4([10, 0, 2, 14]), 32));
        assert_eq!(ours.network_broadcast(24), Ipv4([10, 0, 2, 255]));
        assert_eq!(ours.network_broadcast(20), Ipv4([10, 0, 15, 255]));
        assert_eq!(Ipv4::mask(24), 0xFFFF_FF00);
    }
}
