//! /dev's files (J65): a store with no files of its own, only devices -
//! names whose reads and writes are made up on the spot.
//!
//!   null      reads nothing (the end at once); takes every write
//!   zero      reads zeros, forever; takes every write
//!   full      reads zeros; every write fails: no room (C's ENOSPC) -
//!             for testing what a program does when the disk is full
//!   random    random bytes, forever, from the kernel's generator;
//!   urandom   the same (one generator, which never blocks: J2)
//!
//! What a device reads is the same at every offset, and each one's size
//! is 0, as on Unix: a device is not a file that holds bytes. Nothing in
//! the directory can be made, removed or renamed.
//!
//! The random bytes come from whoever made the store (`DevStore::new`):
//! the `dev` service passes the kernel's generator, a test its own. This
//! crate never calls the kernel itself. If the generator fails, the read
//! fails (C's EIO): zeros in place of random bytes would be a key nobody
//! knew was weak.

use crate::protocol::{Kind, Status};
use crate::{names, valid_directory, Store};

/// One of the devices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Device {
    Null,
    Zero,
    Full,
    Random,
}

/// The devices, in name order (the order LIST gives them in).
const DEVICES: [(&str, Device); 5] = [
    ("full", Device::Full),
    ("null", Device::Null),
    ("random", Device::Random),
    ("urandom", Device::Random),
    ("zero", Device::Zero),
];

/// What a path names here.
enum Named {
    Root,
    Device(Device),
}

/// The devices, as a Store.
pub struct DevStore {
    random: fn(&mut [u8]) -> bool,
}

impl DevStore {
    /// The devices, with `random` to fill a buffer with random bytes:
    /// false if it could not.
    pub fn new(random: fn(&mut [u8]) -> bool) -> Self {
        DevStore { random }
    }

    /// The device names, in order: for whoever wants to say what is here.
    pub fn names() -> impl Iterator<Item = &'static str> {
        DEVICES.iter().map(|&(name, _)| name)
    }
}

/// What `path` names: the root, a device, or why neither.
fn find(path: &str) -> Result<Named, Status> {
    if !valid_directory(path) {
        return Err(Status::BadName);
    }
    let mut steps = names(path);
    let Some(first) = steps.next() else { return Ok(Named::Root) };
    let device = DEVICES.iter().find(|&&(name, _)| name == first).map(|&(_, device)| device);
    match (device, steps.next()) {
        (Some(device), None) => Ok(Named::Device(device)),
        // A device is no directory: nothing is inside it.
        (Some(_), Some(_)) => Err(Status::NotDirectory),
        (None, _) => Err(Status::NotFound),
    }
}

/// A device, or why `path` is not one: a directory, nothing there, or -
/// for a request that would make it - not a place anything can be made.
fn device(path: &str) -> Result<Device, Status> {
    match find(path)? {
        Named::Device(device) => Ok(device),
        Named::Root => Err(Status::IsDirectory),
    }
}

/// Why nothing can be made, removed or renamed at `path`: as for a
/// read-only channel (C's EROFS) where the name is good, and the usual
/// reasons where it is not.
fn unchangeable(path: &str) -> Status {
    match find(path) {
        Ok(_) | Err(Status::NotFound) => Status::ReadOnly,
        Err(status) => status,
    }
}

impl Store for DevStore {
    fn list(&mut self, directory: &str, from: usize, each: &mut dyn FnMut(&str, Kind, u64) -> bool) -> Result<(), Status> {
        match find(directory)? {
            Named::Device(_) => Err(Status::NotDirectory),
            Named::Root => {
                for &(name, _) in DEVICES.iter().skip(from) {
                    if !each(name, Kind::File, 0) {
                        break;
                    }
                }
                Ok(())
            }
        }
    }

    fn stat(&mut self, path: &str) -> Result<(Kind, u64), Status> {
        Ok(match find(path)? {
            Named::Root => (Kind::Directory, 0),
            Named::Device(_) => (Kind::File, 0),
        })
    }

    fn stamp(&mut self, path: &str) -> Result<u64, Status> {
        find(path).map(|_| 0)
    }

    /// `touch /dev/null` works, as on Unix, and changes nothing.
    fn touch(&mut self, name: &str) -> Result<(), Status> {
        device(name).map(|_| ())
    }

    fn make_directory(&mut self, path: &str) -> Result<(), Status> {
        Err(match find(path) {
            Ok(_) => Status::Exists,
            Err(_) => unchangeable(path),
        })
    }

    fn read(&mut self, name: &str, _offset: u64, out: &mut [u8]) -> Result<usize, Status> {
        match device(name)? {
            Device::Null => Ok(0),
            Device::Zero | Device::Full => {
                out.fill(0);
                Ok(out.len())
            }
            Device::Random if (self.random)(out) => Ok(out.len()),
            Device::Random => Err(Status::DiskError),
        }
    }

    /// Taken and forgotten - the size stays 0 - except by `full`.
    fn write(&mut self, name: &str, _offset: u64, _data: &[u8]) -> Result<u64, Status> {
        match find(name) {
            Ok(Named::Device(Device::Full)) => Err(Status::Full),
            Ok(Named::Device(_)) => Ok(0),
            Ok(Named::Root) => Err(Status::IsDirectory),
            Err(_) => Err(unchangeable(name)),
        }
    }

    /// Emptying a device (O_TRUNC) changes nothing, and is allowed: a
    /// program's `> /dev/null` opens it that way.
    fn truncate(&mut self, name: &str, _length: u64) -> Result<(), Status> {
        match find(name) {
            Ok(Named::Device(_)) => Ok(()),
            Ok(Named::Root) => Err(Status::IsDirectory),
            Err(_) => Err(unchangeable(name)),
        }
    }

    fn remove(&mut self, name: &str) -> Result<(), Status> {
        find(name)?;
        Err(Status::ReadOnly)
    }

    fn rename(&mut self, from: &str, _to: &str) -> Result<(), Status> {
        find(from)?;
        Err(Status::ReadOnly)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::String;
    use alloc::vec::Vec;

    /// Not random at all: sevens, so a test can see where they went.
    fn sevens(out: &mut [u8]) -> bool {
        out.fill(7);
        true
    }

    /// A generator that fails.
    fn broken(_: &mut [u8]) -> bool {
        false
    }

    #[test]
    fn lists_the_devices_in_order() {
        let mut dev = DevStore::new(sevens);
        let mut seen = Vec::new();
        dev.list("", 0, &mut |name, kind, size| {
            seen.push((String::from(name), kind, size));
            true
        })
        .unwrap();
        let names: Vec<&str> = seen.iter().map(|(name, _, _)| name.as_str()).collect();
        assert_eq!(names, ["full", "null", "random", "urandom", "zero"]);
        assert!(seen.iter().all(|&(_, kind, size)| kind == Kind::File && size == 0));
        let mut rest = Vec::new();
        dev.list("", 3, &mut |name, _, _| {
            rest.push(String::from(name));
            true
        })
        .unwrap();
        assert_eq!(rest, ["urandom", "zero"]);
        assert_eq!(dev.list("null", 0, &mut |_, _, _| true), Err(Status::NotDirectory));
        assert_eq!(dev.list("nothing", 0, &mut |_, _, _| true), Err(Status::NotFound));
    }

    #[test]
    fn each_device_reads_and_writes_as_unix_says() {
        let mut dev = DevStore::new(sevens);
        let mut out = [9u8; 16];
        assert_eq!(dev.read("null", 0, &mut out), Ok(0));
        assert_eq!(dev.read("zero", 1_000_000, &mut out), Ok(16));
        assert_eq!(out, [0; 16]);
        out.fill(9);
        assert_eq!(dev.read("full", 0, &mut out), Ok(16));
        assert_eq!(out, [0; 16]);
        assert_eq!(dev.read("urandom", 0, &mut out), Ok(16));
        assert_eq!(out, [7; 16]);
        out.fill(9);
        assert_eq!(dev.read("random", 5, &mut out), Ok(16));
        assert_eq!(out, [7; 16]);
        for name in ["null", "zero", "random", "urandom"] {
            assert_eq!(dev.write(name, 0, b"gone"), Ok(0), "{name}");
            assert_eq!(dev.truncate(name, 0), Ok(()), "{name}");
            assert_eq!(dev.stat(name), Ok((Kind::File, 0)), "{name}");
        }
        assert_eq!(dev.write("full", 0, b"no room"), Err(Status::Full));
        assert_eq!(dev.touch("null"), Ok(()));
        // No random bytes: no read, rather than zeros.
        assert_eq!(DevStore::new(broken).read("urandom", 0, &mut out), Err(Status::DiskError));
    }

    #[test]
    fn nothing_can_be_made_removed_or_renamed() {
        let mut dev = DevStore::new(sevens);
        assert_eq!(dev.stat(""), Ok((Kind::Directory, 0)));
        assert_eq!(dev.stat("nothing"), Err(Status::NotFound));
        assert_eq!(dev.stat("null/x"), Err(Status::NotDirectory));
        assert_eq!(dev.write("new", 0, b"x"), Err(Status::ReadOnly));
        assert_eq!(dev.truncate("new", 0), Err(Status::ReadOnly));
        assert_eq!(dev.make_directory("d"), Err(Status::ReadOnly));
        assert_eq!(dev.make_directory("null"), Err(Status::Exists));
        assert_eq!(dev.remove("null"), Err(Status::ReadOnly));
        assert_eq!(dev.remove("nothing"), Err(Status::NotFound));
        assert_eq!(dev.rename("zero", "nought"), Err(Status::ReadOnly));
        assert_eq!(dev.read("", 0, &mut [0; 4]), Err(Status::IsDirectory));
        assert_eq!(dev.write("..", 0, b"x"), Err(Status::BadName));
    }
}
