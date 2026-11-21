//! The Internet checksum (RFC 1071): the one sum IPv4, ICMP and UDP all
//! use.
//!
//! The bytes are taken two at a time as 16-bit numbers and added up;
//! whatever overflows 16 bits is added back in at the bottom ("end-
//! around carry"); the result is inverted. The sender puts that in the
//! header. A receiver adds up the same bytes *with* the checksum in
//! place, and must get 0xFFFF - which `finish` turns into 0.
//!
//! It is a weak check (two swapped words go unnoticed), and cheap,
//! which in 1981 was the point.

/// Adds `data` to a running sum. An odd last byte counts as the high
/// half of a word whose low half is zero - so only the last piece of
/// several may have an odd length.
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

/// Folds the carries back in and inverts: the checksum to put in a
/// header - or, over data that includes its checksum, 0 if it is whole.
pub fn finish(mut sum: u32) -> u16 {
    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !(sum as u16)
}

/// The checksum of one piece of data.
pub fn of(data: &[u8]) -> u16 {
    finish(add(0, data))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_example_in_the_rfc() {
        // RFC 1071, section 3: these eight bytes sum to 0xddf2.
        let data = [0x00, 0x01, 0xf2, 0x03, 0xf4, 0xf5, 0xf6, 0xf7];
        assert_eq!(of(&data), !0xddf2);
    }

    #[test]
    fn a_real_header() {
        // An IPv4 header often used as the worked example: its checksum
        // field (bytes 10 and 11) is 0xb861.
        let mut header = [0x45, 0x00, 0x00, 0x73, 0x00, 0x00, 0x40, 0x00, 0x40, 0x11, 0xb8, 0x61, 0xc0, 0xa8, 0x00, 0x01, 0xc0, 0xa8, 0x00, 0xc7];
        assert_eq!(of(&header), 0, "whole, with its checksum in place");
        header[10] = 0;
        header[11] = 0;
        assert_eq!(of(&header), 0xb861);
        header[3] ^= 1;
        assert_ne!(of(&header), 0xb861, "one bit changed");
    }

    #[test]
    fn an_odd_byte_and_pieces() {
        assert_eq!(of(&[0x12]), !0x1200);
        assert_eq!(of(&[0x12, 0x34, 0x56]), !(0x1234 + 0x5600));
        // In pieces (the first of even length) as in one.
        let data: Vec<u8> = (0..=254u8).collect();
        assert_eq!(finish(add(add(0, &data[..100]), &data[100..])), of(&data));
        // Carries are folded, however many - also the carry that
        // folding a carry makes: 0xFFFF + 0xFFFF + 1 is 0x1FFFF, which
        // folds to 0x10000, which folds to 1.
        assert_eq!(of(&[0xFF; 4000]), 0);
        assert_eq!(of(&[0xFF, 0xFF, 0xFF, 0xFF, 0x00, 0x01]), !1);
        assert_eq!(of(&[]), 0xFFFF);
    }
}
