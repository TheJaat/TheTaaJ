//! fscheck: a test of the file service from ring 3, as a client.
//!
//! The boot test types a few short commands at the shell; this checks
//! what a few words cannot reach: files many messages long, reads that
//! end exactly on a message boundary, listings that take several
//! replies, every refusal, and requests a careless or hostile client
//! could send - with no reply channel, with extra handles, or garbage.
//!
//! The kernel's self-test runs it against a fresh `fs`, twice, and then
//! checks that no channel and no frame was left behind. Exit code 0 if
//! everything held, else the line of the failed check.

#![no_std]
#![no_main]

use core::fmt::Write;

use ramfs::protocol::{Status, MESSAGE_MAX, NAME_MAX, READ_MAX};
use taaj::files::{FileError, Files};
use taaj::{println, Buffer, Error};

taaj::program!(main);

macro_rules! check {
    ($cond:expr, $what:literal) => {
        if !$cond {
            println!("[fscheck] FAILED: {}", $what);
            return line!() as i64;
        }
    };
}

/// Byte `n` of a test file: a pattern that shows misplaced chunks.
fn pattern(n: usize) -> u8 {
    (n * 7 + n / 251) as u8
}

/// Reads `name` back and compares it with the pattern.
fn matches(files: &Files, name: &str, length: usize) -> bool {
    let mut at = 0;
    let mut good = true;
    let read = files.read(name, |piece| {
        good &= piece.iter().enumerate().all(|(i, &b)| b == pattern(at + i));
        at += piece.len();
    });
    good && read == Ok(length as u64) && at == length
}

fn refused<T>(status: Status) -> Result<T, FileError> {
    Err(FileError::Refused(status))
}

fn main() -> i64 {
    let requests = taaj::bootstrap();
    let files = Files::new(requests);
    let mut data = [0u8; 5000];
    for (n, byte) in data.iter_mut().enumerate() {
        *byte = pattern(n);
    }

    // 1. A file five messages long, written and read back in pieces.
    check!(files.write("big", &data) == Ok(5000), "write a long file");
    check!(files.size("big") == Ok(5000), "its size");
    check!(matches(&files, "big", 5000), "read it back");
    // Shorter again: write replaces the contents, it does not overlay.
    check!(files.write("big", &data[..3000]) == Ok(3000), "rewrite shorter");
    check!(matches(&files, "big", 3000), "only the new contents");

    // 2. Exactly two reads' worth: the reader must ask once more and get
    //    nothing, not stop early or read garbage.
    check!(files.write("edge", &data[..2 * READ_MAX]) == Ok(2 * READ_MAX as u64), "write two reads' worth");
    check!(matches(&files, "edge", 2 * READ_MAX), "read across the boundary");

    // 3. Append, in pieces, keeps adding at the end.
    check!(files.append("log", &data[..1500]) == Ok(1500), "append creates");
    check!(files.append("log", &data[1500..4000]) == Ok(4000), "append adds");
    check!(matches(&files, "log", 4000), "appended pieces in order");
    check!(files.write("empty", &[]) == Ok(0), "an empty write still makes the file");

    // 4. A listing longer than one reply.
    let mut many_bytes = 0;
    for n in 0..100 {
        let mut name = Buffer::<NAME_MAX>::new();
        let _ = write!(name, "a-file-with-a-rather-long-name-number-{n:03}");
        let name = core::str::from_utf8(name.as_bytes()).unwrap_or("");
        check!(files.write(name, name.as_bytes()).is_ok(), "write many files");
        many_bytes += name.len() as u64;
    }
    let (mut count, mut previous_ok, mut sizes) = (0, true, 0);
    let mut last = Buffer::<NAME_MAX>::new();
    let listed = files.list(|name, size| {
        // Name order, and every name once.
        previous_ok &= name.as_bytes() > last.as_bytes();
        last = Buffer::new();
        let _ = last.write_str(name);
        count += 1;
        sizes += size;
    });
    check!(listed == Ok(104) && count == 104, "every file listed, across several replies");
    check!(previous_ok, "listed in order, none twice");
    check!(sizes == 3000 + 2 * READ_MAX as u64 + 4000 + many_bytes, "sizes listed");

    // 5. Refusals.
    check!(files.size("nothing") == refused(Status::NotFound), "missing file");
    check!(files.read("nothing", |_| {}) == refused(Status::NotFound), "read a missing file");
    check!(files.remove("nothing") == refused(Status::NotFound), "remove a missing file");
    check!(files.write("a/b", b"x") == refused(Status::BadName), "a slash");
    check!(files.write("", b"x") == refused(Status::BadName), "an empty name");
    let long = [b'n'; NAME_MAX + 1];
    let long = core::str::from_utf8(&long).unwrap_or("");
    check!(files.write(long, b"x") == refused(Status::BadName), "an over-long name");
    let mut huge = 0;
    while files.append("huge", &data).is_ok() {
        huge += 1;
        check!(huge < 1000, "a file grew without limit");
    }
    check!(files.append("huge", &data) == refused(Status::TooBig), "a file's size limit");
    check!(files.remove("huge") == Ok(()), "remove the huge file");

    // 6. Requests from a careless client. The service must stay up, and
    //    not keep handles it was not meant to.
    let mut message = [0u8; MESSAGE_MAX];
    let request = ramfs::protocol::Request::new(ramfs::protocol::STAT, "big", 0);
    let length = request.encode(&mut message).unwrap_or(0);
    check!(taaj::send(requests, &message[..length], &[]) == Ok(()), "a request with no reply channel");
    let Ok((mine, theirs)) = taaj::channel_create() else { return 100 };
    let Ok((mine2, theirs2)) = taaj::channel_create() else { return 100 };
    check!(taaj::send(requests, &message[..length], &[theirs, theirs2]) == Ok(()), "a request with two handles");
    let mut reply = [0u8; 16];
    check!(taaj::receive(mine, &mut reply, &mut []) == Err(Error::PeerClosed), "fs closed the extra handles");
    check!(taaj::receive(mine2, &mut reply, &mut []) == Err(Error::PeerClosed), "fs closed the extra handles");
    let _ = (taaj::close(mine), taaj::close(mine2));
    // Garbage, with a reply channel: an answer saying so.
    let Ok((mine, theirs)) = taaj::channel_create() else { return 100 };
    check!(taaj::send(requests, b"\x02", &[theirs]) == Ok(()), "send garbage");
    let answer = taaj::receive(mine, &mut reply, &mut []);
    check!(answer == Ok((1, 0)) && reply[0] == Status::BadRequest as u8, "garbage refused");
    let _ = taaj::close(mine);
    check!(files.size("big") == Ok(3000), "the service is still up");

    // 7. Clean up: an empty file system.
    let mut names = [[0u8; NAME_MAX]; 4];
    let mut lengths = [0usize; 4];
    loop {
        let mut n = 0;
        let _ = files.list(|name, _| {
            if n < names.len() {
                names[n][..name.len()].copy_from_slice(name.as_bytes());
                lengths[n] = name.len();
                n += 1;
            }
        });
        if n == 0 {
            break;
        }
        for (name, &length) in names.iter().zip(&lengths).take(n) {
            let name = core::str::from_utf8(&name[..length]).unwrap_or("");
            check!(files.remove(name) == Ok(()), "remove");
        }
    }
    check!(files.list(|_, _| {}) == Ok(0), "nothing left");

    println!("[fscheck] long files, boundaries, appends, 104-file listing, refusals, careless requests: all passed");
    0
}
