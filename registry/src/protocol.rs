//! The registry's messages. The first byte says which.
//!
//! On the admin channel, from init - each with one handle, no reply:
//!
//!   [REGISTER] [rights: u32 LE] [name]      + the service's handle
//!   [NAMESPACE] [label] 0 [name] 0 ...      + the receiving end of a
//!                                             client's channel
//!
//! `rights` are the rights a client gets on its copy of the handle.
//! `label` says whose namespace it is - "shell" - for the log only.
//! Registering a name again replaces the handle; a namespace with a
//! label already in use replaces that namespace.
//!
//! On a client channel - each with one handle, the sending end of a
//! channel for the reply, as the file service's requests have:
//!
//!   [LOOKUP] [name]    reply: [OK] + a handle, or [NOT_FOUND]
//!   [LIST]             reply: [OK] [name] 0 [name] 0 ...
//!   [NARROW] [label] 0 [new] 0 [old] 0 [new] 0 [old] 0 ...
//!                      reply: [OK] + the sending end of a channel to
//!                      a new namespace, or [NOT_FOUND]
//!
//! In NARROW each pair is a name the new namespace is to have, and the
//! name in the asker's own namespace that it is to mean. NOT_FOUND if
//! one of the old names is not in the asker's namespace: nobody can
//! give what they cannot reach. `label` is for the log, as init's is.
//!
//! And a test:
//!
//!   [CRASH]            a test: the registry ends, as if it had failed
//!
//! Lists of names travel as arguments do (abi::arguments): each
//! followed by a zero byte.

use alloc::vec::Vec;

pub const REGISTER: u8 = 1;
pub const NAMESPACE: u8 = 2;
pub const LOOKUP: u8 = 3;
pub const LIST: u8 = 4;
pub const CRASH: u8 = 5;
pub const NARROW: u8 = 6;

/// A reply's first byte.
pub const OK: u8 = 0;
/// No such name - in this namespace. The reply does not say whether
/// the name exists for someone else: for this client it does not.
pub const NOT_FOUND: u8 = 1;
/// Not a request the registry understands.
pub const BAD_REQUEST: u8 = 2;

/// Most bytes in a name.
pub const NAME_MAX: usize = 32;

/// Whether `name` can name a service: 1 to NAME_MAX of a-z, 0-9, '.'
/// and '-'. Strict, because names are compared byte for byte and shown
/// to the user: no two names should look alike.
pub fn valid_name(name: &str) -> bool {
    (1..=NAME_MAX).contains(&name.len())
        && name.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'.' || byte == b'-')
}

/// What init tells the registry.
#[derive(Debug, PartialEq, Eq)]
pub enum Admin<'a> {
    Register { name: &'a str, rights: u32 },
    Namespace { label: &'a str, names: Vec<&'a str> },
}

impl<'a> Admin<'a> {
    /// Writes the message into `out`; its length, or None if a name is
    /// not valid or it does not fit.
    pub fn encode(&self, out: &mut [u8]) -> Option<usize> {
        match self {
            Admin::Register { name, rights } => {
                let length = 5 + name.len();
                if !valid_name(name) || out.len() < length {
                    return None;
                }
                out[0] = REGISTER;
                out[1..5].copy_from_slice(&rights.to_le_bytes());
                out[5..length].copy_from_slice(name.as_bytes());
                Some(length)
            }
            Admin::Namespace { label, names } => {
                if !valid_name(label) || !names.iter().all(|name| valid_name(name)) || out.is_empty() {
                    return None;
                }
                out[0] = NAMESPACE;
                // `once` makes an iterator of one item, `chain` puts
                // the names after it: the label, then every name.
                let words = core::iter::once(*label).chain(names.iter().copied());
                abi::arguments::encode(words, &mut out[1..]).map(|length| length + 1)
            }
        }
    }

    /// Reads a message. None if it is not one, or a name in it is not
    /// valid.
    pub fn decode(message: &'a [u8]) -> Option<Admin<'a>> {
        let (&kind, rest) = message.split_first()?;
        match kind {
            REGISTER => {
                let rights = u32::from_le_bytes(rest.get(..4)?.try_into().ok()?);
                let name = name(&rest[4..])?;
                Some(Admin::Register { name, rights })
            }
            NAMESPACE => {
                // `collect` into an Option: None if any name was bad.
                let mut words = abi::arguments::decode(rest).map(name).collect::<Option<Vec<&str>>>()?.into_iter();
                let label = words.next()?;
                Some(Admin::Namespace { label, names: words.collect() })
            }
            _ => None,
        }
    }
}

/// What a client asks.
#[derive(Debug, PartialEq, Eq)]
pub enum Request<'a> {
    Lookup(&'a str),
    List,
    Crash,
    /// A new namespace: its label, and its names - each with the name
    /// in the asker's namespace that it is to mean.
    Narrow { label: &'a str, names: Vec<(&'a str, &'a str)> },
}

impl<'a> Request<'a> {
    pub fn encode(&self, out: &mut [u8]) -> Option<usize> {
        let (kind, name) = match self {
            Request::Lookup(name) => (LOOKUP, *name),
            Request::List => (LIST, ""),
            Request::Crash => (CRASH, ""),
            Request::Narrow { label, names } => {
                let valid = valid_name(label) && names.iter().all(|(new, old)| valid_name(new) && valid_name(old));
                if !valid || out.is_empty() {
                    return None;
                }
                out[0] = NARROW;
                // `flat_map` turns each pair into its two names, one
                // after the other: the label, new, old, new, old, ...
                let words = core::iter::once(*label).chain(names.iter().flat_map(|&(new, old)| [new, old]));
                return abi::arguments::encode(words, &mut out[1..]).map(|length| length + 1);
            }
        };
        // Any text of a name's length may be asked for: asking for a
        // name that could never exist gets "not found", like any other.
        let length = 1 + name.len();
        if name.len() > NAME_MAX || out.len() < length {
            return None;
        }
        out[0] = kind;
        out[1..length].copy_from_slice(name.as_bytes());
        Some(length)
    }

    pub fn decode(message: &'a [u8]) -> Option<Request<'a>> {
        let (&kind, rest) = message.split_first()?;
        match (kind, rest) {
            (LOOKUP, rest) if rest.len() <= NAME_MAX => core::str::from_utf8(rest).ok().map(Request::Lookup),
            (LIST, []) => Some(Request::List),
            (CRASH, []) => Some(Request::Crash),
            (NARROW, rest) => {
                let words = abi::arguments::decode(rest).map(name).collect::<Option<Vec<&str>>>()?;
                let (label, names) = words.split_first()?;
                // `as_chunks::<2>` gives the names two at a time, as
                // arrays of two, and whatever is left over at the end:
                // a name with no partner.
                let (pairs, []) = names.as_chunks::<2>() else { return None };
                Some(Request::Narrow { label, names: pairs.iter().map(|&[new, old]| (new, old)).collect() })
            }
            _ => None,
        }
    }
}

/// The bytes as a valid name, if they are one.
fn name(bytes: &[u8]) -> Option<&str> {
    core::str::from_utf8(bytes).ok().filter(|name| valid_name(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_short_plain_and_not_empty() {
        for good in ["files", "files.look", "disk-2", "a", &"x".repeat(NAME_MAX)] {
            assert!(valid_name(good), "{good}");
        }
        for bad in ["", "Files", "my files", "files/look", "café", &"x".repeat(NAME_MAX + 1), "a\0b"] {
            assert!(!valid_name(bad), "{bad:?}");
        }
    }

    #[test]
    fn a_registration_comes_back_as_it_went() {
        let mut out = [0u8; 64];
        let message = Admin::Register { name: "files.look", rights: 0x0102_0304 };
        let length = message.encode(&mut out).unwrap();
        assert_eq!(length, 5 + "files.look".len());
        assert_eq!(Admin::decode(&out[..length]), Some(message));
    }

    #[test]
    fn a_namespace_comes_back_as_it_went() {
        let mut out = [0u8; 128];
        let message = Admin::Namespace { label: "shell", names: vec!["keys", "display", "files"] };
        let length = message.encode(&mut out).unwrap();
        assert_eq!(Admin::decode(&out[..length]), Some(message));
        // One with no names at all is allowed: a client that may ask
        // for nothing.
        let none = Admin::Namespace { label: "nobody", names: vec![] };
        let length = none.encode(&mut out).unwrap();
        assert_eq!(Admin::decode(&out[..length]), Some(none));
    }

    #[test]
    fn bad_admin_messages_are_not_messages() {
        let mut out = [0u8; 64];
        assert_eq!(Admin::Register { name: "Bad Name", rights: 0 }.encode(&mut out), None);
        assert_eq!(Admin::Register { name: "files", rights: 0 }.encode(&mut out[..7]), None);
        assert_eq!(Admin::Namespace { label: "shell", names: vec!["ok", "NOT"] }.encode(&mut out), None);
        assert_eq!(Admin::decode(&[]), None);
        assert_eq!(Admin::decode(&[REGISTER, 1, 2, 3]), None); // rights cut short
        assert_eq!(Admin::decode(&[REGISTER, 0, 0, 0, 0]), None); // no name
        assert_eq!(Admin::decode(&[REGISTER, 0, 0, 0, 0, b'A']), None);
        assert_eq!(Admin::decode(&[NAMESPACE]), None); // no label
        assert_eq!(Admin::decode(&[NAMESPACE, b's', 0, b'B', 0]), None);
        assert_eq!(Admin::decode(&[LOOKUP, b'x']), None); // a client's request
        assert_eq!(Admin::decode(&[99]), None);
    }

    #[test]
    fn requests_come_back_as_they_went() {
        let mut out = [0u8; 64];
        for request in [Request::Lookup("files"), Request::Lookup(""), Request::List, Request::Crash] {
            let length = request.encode(&mut out).unwrap();
            assert_eq!(Request::decode(&out[..length]), Some(request));
        }
        // A name that could never be registered may still be asked for.
        let length = Request::Lookup("No Such").encode(&mut out).unwrap();
        assert_eq!(Request::decode(&out[..length]), Some(Request::Lookup("No Such")));
    }

    #[test]
    fn a_narrowing_comes_back_as_it_went() {
        let mut out = [0u8; 128];
        let request = Request::Narrow { label: "ls", names: vec![("files", "files.look"), ("display", "display")] };
        let length = request.encode(&mut out).unwrap();
        assert_eq!(Request::decode(&out[..length]), Some(request));
        // No names: a namespace with nothing in it.
        let none = Request::Narrow { label: "given", names: vec![] };
        let length = none.encode(&mut out).unwrap();
        assert_eq!(Request::decode(&out[..length]), Some(none));
    }

    #[test]
    fn a_bad_narrowing_is_not_a_request() {
        let mut out = [0u8; 128];
        assert_eq!(Request::Narrow { label: "Bad", names: vec![] }.encode(&mut out), None);
        assert_eq!(Request::Narrow { label: "ls", names: vec![("files", "NOT")] }.encode(&mut out), None);
        assert_eq!(Request::Narrow { label: "ls", names: vec![("", "files")] }.encode(&mut out), None);
        assert_eq!(Request::decode(&[NARROW]), None); // no label
        assert_eq!(Request::decode(&[NARROW, b'l', 0, b'a', 0]), None); // a name with no partner
        assert_eq!(Request::decode(&[NARROW, b'l', 0, b'a', 0, b'B', 0]), None);
    }

    #[test]
    fn bad_requests_are_not_requests() {
        let mut out = [0u8; 64];
        assert_eq!(Request::Lookup(&"x".repeat(NAME_MAX + 1)).encode(&mut out), None);
        assert_eq!(Request::decode(&[]), None);
        assert_eq!(Request::decode(&[LIST, 1]), None);
        assert_eq!(Request::decode(&[CRASH, 0]), None);
        assert_eq!(Request::decode(&[LOOKUP, 0xff]), None); // not UTF-8
        assert_eq!(Request::decode(&[REGISTER, 0, 0, 0, 0, b'x']), None); // init's, not a client's
        let mut long = vec![LOOKUP];
        long.extend_from_slice(&[b'x'; NAME_MAX + 1]);
        assert_eq!(Request::decode(&long), None);
    }
}
