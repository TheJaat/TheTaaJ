//! How long the number at the start of some text is, as C's strtod
//! reads numbers, and whether what it came to is out of range. No
//! system in it: it is tested on the host, alone
//! (`rustc --test libc/src/number.rs`, in `make libc-test`).

/// The length of the number at the start of `text`, as strtod reads
/// one: a sign, then "inf", "infinity" or "nan" (any case), or digits
/// with at most one point among them and at least one digit, then an
/// exponent - 'e' or 'E', a sign, digits - if one is really there.
/// Zero if there is no number. Hexadecimal ("0x1p3") is not read: the
/// "0" is, and the rest is left.
pub fn number_length(text: &[u8]) -> usize {
    let mut at = 0;
    if matches!(text.first(), Some(b'+' | b'-')) {
        at = 1;
    }
    let rest = &text[at..];
    for word in [&b"infinity"[..], b"inf", b"nan"] {
        if rest.len() >= word.len() && rest[..word.len()].eq_ignore_ascii_case(word) {
            return at + word.len();
        }
    }
    let mut digits = 0;
    while text.get(at).is_some_and(u8::is_ascii_digit) {
        at += 1;
        digits += 1;
    }
    if text.get(at) == Some(&b'.') {
        at += 1;
        while text.get(at).is_some_and(u8::is_ascii_digit) {
            at += 1;
            digits += 1;
        }
    }
    if digits == 0 {
        return 0;
    }
    if matches!(text.get(at), Some(b'e' | b'E')) {
        let mut exponent = at + 1;
        if matches!(text.get(exponent), Some(b'+' | b'-')) {
            exponent += 1;
        }
        if text.get(exponent).is_some_and(u8::is_ascii_digit) {
            at = exponent;
            while text.get(at).is_some_and(u8::is_ascii_digit) {
                at += 1;
            }
        }
    }
    at
}

/// True if a finite number came out infinite, or digits that were not
/// all zero came out zero: out of the type's range, which C reports in
/// errno as ERANGE (the value is still the nearest one: infinity, or 0).
pub fn out_of_range(infinite: bool, zero: bool, text: &[u8]) -> bool {
    let named = text.iter().any(|b| b.is_ascii_alphabetic() && !matches!(b, b'e' | b'E'));
    let nonzero_digit = text.iter().take_while(|b| !matches!(b, b'e' | b'E')).any(|b| matches!(b, b'1'..=b'9'));
    (infinite && !named) || (zero && nonzero_digit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimal_numbers() {
        for (text, length) in [
            ("1", 1),
            ("12.5", 4),
            ("-0.25x", 5),
            ("+.5", 3),
            ("5.", 2),
            ("1e10", 4),
            ("1.5E-3;", 6),
            ("2e+", 1),
            ("7e", 1),
            ("3.0e5.1", 5),
        ] {
            assert_eq!(number_length(text.as_bytes()), length, "{text}");
        }
    }

    #[test]
    fn names_and_nothing() {
        assert_eq!(number_length(b"inf"), 3);
        assert_eq!(number_length(b"-Infinity!"), 9);
        assert_eq!(number_length(b"NaN"), 3);
        assert_eq!(number_length(b"info"), 3);
        for text in ["", ".", "-", "+.", "e5", "x1"] {
            assert_eq!(number_length(text.as_bytes()), 0, "{text}");
        }
        // Hexadecimal: only the 0 is read.
        assert_eq!(number_length(b"0x1p3"), 1);
    }

    #[test]
    fn what_is_out_of_range() {
        assert!(out_of_range(true, false, b"1e999"));
        assert!(!out_of_range(true, false, b"inf"));
        assert!(out_of_range(false, true, b"1e-999"));
        assert!(!out_of_range(false, true, b"0.000"));
        assert!(!out_of_range(false, true, b"0e-999"));
        assert!(!out_of_range(false, false, b"1.5"));
    }
}
