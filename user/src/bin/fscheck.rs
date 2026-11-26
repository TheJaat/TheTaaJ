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

use fileserver::protocol::{Status, MESSAGE_MAX, NAME_MAX, READ_MAX};
use taaj::files::{FileError, Files, Kind};
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

/// Whether the file `name` holds exactly `expected`.
fn matches_bytes(files: &Files, name: &str, expected: &[u8]) -> bool {
    let mut at = 0;
    let mut good = true;
    let read = files.read(name, |piece| {
        good &= expected.get(at..at + piece.len()) == Some(piece);
        at += piece.len();
    });
    good && read == Ok(expected.len() as u64)
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
    let listed = files.list("", |name, _, size| {
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
    check!(files.write("a/b", b"x") == refused(Status::NotFound), "a path through nothing");
    check!(files.write("big/b", b"x") == refused(Status::NotDirectory), "a path through a file");
    // ".." never reaches the service from here: the client works it
    // out first (section 11). That the service itself refuses one is
    // checked on the host, where a raw request can be made.
    check!(files.size("a/../nothing") == refused(Status::NotFound), "a path that climbs is resolved, then looked up");
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
    let request = fileserver::protocol::Request::new(fileserver::protocol::STAT, "big", 0);
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

    // 7. A service that never answers: a channel nobody reads stands
    //    in for it. The client must give up when its patience ends -
    //    not sooner, and not never.
    match taaj::channel_create() {
        Ok((unread, silent)) => {
            let deaf = Files::with_patience(silent, 10);
            let start = taaj::ticks();
            let answer = deaf.list("", |_, _, _| {});
            let waited = taaj::ticks() - start;
            let _ = taaj::close(silent);
            let _ = taaj::close(unread);
            check!(answer == Err(FileError::Unreachable(Error::TimedOut)), "a silent service: timed out");
            check!((10..=13).contains(&waited), "a silent service: after 10 ticks");
        }
        Err(_) => check!(false, "a channel for the silent service"),
    }

    // 8. The second channel is for reading: everything that looks
    //    works, everything that would change is refused - and changes
    //    nothing.
    let Some(look) = taaj::startup_handle(1).map(Files::new) else {
        println!("[fscheck] FAILED: no read-only channel");
        return line!() as i64;
    };
    check!(files.read_only() == Ok(false), "the first channel changes files");
    check!(look.read_only() == Ok(true), "the second only reads");
    check!(files.write("guarded", &data[..3000]) == Ok(3000), "a file to guard");
    check!(matches(&look, "guarded", 3000), "read through the read-only channel");
    check!(look.size("guarded") == Ok(3000), "its size, too");
    let mut seen = false;
    check!(look.list("", |name, _, _| seen |= name == "guarded").is_ok() && seen, "and the listing");
    check!(look.write("guarded", b"x") == refused(Status::ReadOnly), "no write");
    check!(look.append("guarded", b"x") == refused(Status::ReadOnly), "no append");
    check!(look.remove("guarded") == refused(Status::ReadOnly), "no remove");
    check!(look.write("sneaked", b"x") == refused(Status::ReadOnly), "no new file");
    check!(look.stall(1) == Err(FileError::Refused(Status::ReadOnly)), "no stalling the service");
    check!(matches(&files, "guarded", 3000), "unchanged after all that");
    check!(files.size("sneaked") == refused(Status::NotFound), "and nothing was made");

    // 9. A stall: answered at once, then the service is silent - a
    //    client without the patience gives up, one with it is served.
    let hasty = Files::with_patience(requests, 5);
    check!(files.stall(20) == Ok(()), "the stall itself is answered");
    check!(hasty.size("guarded") == Err(FileError::Unreachable(Error::TimedOut)), "silent while stalled");
    check!(files.size("guarded") == Ok(3000), "and back afterwards");
    check!(files.stall(1_000_000) == refused(Status::BadRequest), "a stall has a limit");

    // 10. Directories: files in them, by path; what a directory is not;
    //     and that only an empty one can be removed.
    check!(files.make_directory("d") == Ok(()), "make a directory");
    check!(files.make_directory("d/inner") == Ok(()), "and one inside it");
    check!(files.make_directory("d") == refused(Status::Exists), "not twice");
    check!(files.make_directory("big") == refused(Status::Exists), "not where a file is");
    check!(files.make_directory("nowhere/x") == refused(Status::NotFound), "not in a directory that is not there");
    check!(look.make_directory("sneaked") == refused(Status::ReadOnly), "not through the read-only channel");
    check!(files.write("d/inner/deep", &data[..3000]) == Ok(3000), "a file two directories down");
    check!(matches(&files, "/d//inner/deep", 3000), "read back, by a path with spare slashes");
    check!(files.append("d/log", &data[..10]) == Ok(10), "append in a directory");
    check!(files.stat("d") == Ok((Kind::Directory, 0)), "a directory, by stat");
    check!(files.stat("") == Ok((Kind::Directory, 0)), "the root is a directory");
    check!(files.stat("d/inner/deep") == Ok((Kind::File, 3000)), "a file, by stat");
    check!(files.size("d") == refused(Status::IsDirectory), "a directory has no size");
    check!(files.write("d", b"x") == refused(Status::IsDirectory), "a directory is not written");
    check!(files.read("d", |_| {}) == refused(Status::IsDirectory), "nor read");
    let (mut inner, mut log, mut others) = (false, false, 0);
    let listed = files.list("d", |name, kind, size| match (name, kind, size) {
        ("inner", Kind::Directory, 0) => inner = true,
        ("log", Kind::File, 10) => log = true,
        _ => others += 1,
    });
    check!(listed == Ok(2) && inner && log && others == 0, "a listing is of one directory");
    let mut deep = false;
    check!(files.list("d/inner", |name, _, size| deep = name == "deep" && size == 3000) == Ok(1) && deep, "and of the one inside");
    check!(files.list("big", |_, _, _| {}) == refused(Status::NotDirectory), "a file cannot be listed");
    check!(files.list("nowhere", |_, _, _| {}) == refused(Status::NotFound), "nor what is not there");

    // 11. The same files, named as someone in d/inner would name them.
    //     The client resolves the name; the service still sees only
    //     whole paths.
    let inside = Files::in_directory(requests, "d/inner");
    check!(matches(&inside, "deep", 3000), "a bare name, from a directory");
    check!(matches(&inside, "./deep", 3000), "with a dot in front");
    check!(inside.size("../log") == Ok(10), "one up");
    check!(inside.size("/d/log") == Ok(10), "from the root, wherever one is");
    check!(inside.size("nowhere/../deep") == Ok(3000), "up from a name is the text's business");
    check!(inside.stat("../..") == Ok((Kind::Directory, 0)), "two up is the root");
    check!(inside.size("../../../../big") == files.size("big"), "and above the root is the root");
    check!(inside.write("../made", b"x") == Ok(1) && files.size("d/made") == Ok(1), "written where the path says");
    check!(inside.append("../made", b"y") == Ok(2), "appended there too");
    check!(inside.make_directory("sub") == Ok(()) && files.stat("d/inner/sub") == Ok((Kind::Directory, 0)), "a directory made from here");
    let mut seen = 0;
    check!(inside.list(".", |name, _, _| seen += usize::from(name == "deep" || name == "sub")) == Ok(2) && seen == 2, "here, listed as a dot");
    check!(inside.list("", |_, _, _| {}) == Ok(2), "and as nothing at all");
    check!(inside.remove("sub") == Ok(()) && inside.remove("../made") == Ok(()), "removed by the same names");
    check!(inside.size("deep/../deep/x") == refused(Status::NotDirectory), "a file is still not a directory");
    check!(files.size("d/inner/deep") == Ok(3000), "nothing was disturbed");

    check!(files.remove("d") == refused(Status::NotEmpty), "a directory with something in it stays");
    check!(files.remove("d/inner") == refused(Status::NotEmpty), "at any depth");
    check!(files.remove("d/inner/deep") == Ok(()), "empty it");
    check!(files.remove("d/inner") == Ok(()), "now it goes");
    check!(files.remove("d/log") == Ok(()), "empty the outer one");
    check!(files.remove("d") == Ok(()), "and it goes too");
    check!(files.stat("d") == refused(Status::NotFound), "gone");

    // 12. Renaming: a new name, another directory, a directory with
    //     what is in it - and what a move may not do.
    check!(files.make_directory("m") == Ok(()) && files.make_directory("m/sub") == Ok(()), "directories to move things in");
    check!(files.write("m/sub/f", &data[..3000]) == Ok(3000), "a file to move");
    check!(files.rename("m/sub/f", "m/sub/g") == Ok(()), "a new name");
    check!(matches(&files, "m/sub/g", 3000) && files.size("m/sub/f") == refused(Status::NotFound), "the same bytes, under it alone");
    check!(files.rename("m/sub/g", "g") == Ok(()) && matches(&files, "g", 3000), "up to the root");
    check!(files.rename("g", "m/sub/g") == Ok(()) && matches(&files, "m/sub/g", 3000), "and back down");
    check!(files.rename("m/sub", "sub") == Ok(()), "a directory moves");
    check!(matches(&files, "sub/g", 3000) && files.list("m", |_, _, _| {}) == Ok(0), "with what is in it");
    check!(files.write("m/full", b"x") == Ok(1), "something in the old directory");
    check!(files.rename("sub", "m") == refused(Status::NotEmpty), "a directory with things in it is not replaced");
    check!(files.rename("sub/g", "m") == refused(Status::IsDirectory), "nor a directory by a file");
    check!(files.rename("sub", "m/full") == refused(Status::NotDirectory), "nor a file by a directory");
    check!(files.write("m/old", b"old") == Ok(3) && files.rename("m/full", "m/old") == Ok(()), "a file replaces a file");
    check!(matches_bytes(&files, "m/old", b"x") && files.size("m/full") == refused(Status::NotFound), "its bytes, under the one name");
    check!(files.remove("m/old") == Ok(()), "the old directory empty again");
    check!(files.rename("sub", "sub/in") == refused(Status::IntoItself), "not into itself");
    check!(files.rename("nothing", "x") == refused(Status::NotFound), "what is not there cannot move");
    check!(files.rename("sub", "nowhere/x") == refused(Status::NotFound), "nor to a directory that is not there");
    check!(files.rename("sub", "big/x") == refused(Status::NotDirectory), "nor through a file");
    check!(look.rename("sub", "x") == refused(Status::ReadOnly), "not through the read-only channel");
    check!(files.rename("sub", "sub") == Ok(()), "to where it is: nothing to do");
    let there = Files::in_directory(requests, "sub");
    check!(there.rename("g", "../m/h") == Ok(()) && matches(&files, "m/h", 3000), "by names from a directory");
    check!(files.remove("m/h") == Ok(()) && files.remove("m") == Ok(()) && files.remove("sub") == Ok(()), "and all of it removed");

    // 13. Change stamps, for make: what changed later is newer, a touch
    //     is a change that leaves the bytes, and a new name is none.
    let stamp = |name: &str| files.stat_stamped(name).map(|(_, _, stamp)| stamp).unwrap_or(0);
    check!(files.write("s.c", b"int s;") == Ok(6) && files.write("s.o", b"object") == Ok(6), "two files, one after the other");
    check!(stamp("s.c") > 0 && stamp("s.o") > stamp("s.c"), "the later one has the higher stamp");
    check!(files.touch("s.c") == Ok(()) && stamp("s.c") > stamp("s.o"), "a touch makes a file the newer");
    check!(matches_bytes(&files, "s.c", b"int s;"), "and changes nothing in it");
    let before = stamp("s.c");
    check!(files.rename("s.c", "t.c") == Ok(()) && stamp("t.c") == before, "a new name keeps the stamp");
    check!(look.touch("s.o") == refused(Status::ReadOnly), "not touched through the read-only channel");
    check!(files.touch("nothere") == refused(Status::NotFound), "nothing to touch");
    check!(files.stat_stamped("").map(|(_, _, stamp)| stamp) == Ok(0), "a directory has no stamp");
    check!(files.remove("t.c") == Ok(()) && files.remove("s.o") == Ok(()), "removed");

    // 14. Clean up: an empty file system.
    let mut names = [[0u8; NAME_MAX]; 4];
    let mut lengths = [0usize; 4];
    loop {
        let mut n = 0;
        let _ = files.list("", |name, _, _| {
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
    check!(files.list("", |_, _, _| {}) == Ok(0), "nothing left");

    println!("[fscheck] long files, boundaries, appends, 104-file listing, refusals, careless requests, a silent service, read-only, a stall, directories, paths from a directory, renaming, change stamps: all passed");
    0
}
