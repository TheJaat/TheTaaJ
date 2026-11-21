//! blkcheck: a test of the block driver from outside it, as a client.
//!
//! Startup handles:
//!   1  the driver's request channel: reads and writes
//!   2  its second channel: reads only
//!
//! The disk it expects is one the boot test made: byte `i` of sector
//! `s` is `original(s, i)`, everywhere - so a sector read from the
//! wrong place, or half of one, shows. It writes `written(s, i)` to a
//! few sectors and reads them back; the boot test then looks at the
//! image file from outside and must find exactly those sectors changed.
//!
//! Run twice against the same disk (the kernel's self-test does), the
//! second run finds the first one's writes where the original pattern
//! was, which is why the sectors it writes are never checked against
//! `original` first.
//!
//! Exit code 0 if everything held, else the line of the failed check.

#![no_std]
#![no_main]

use abi::block::{self, BAD_REQUEST, OUT_OF_RANGE, READ_ONLY, SECTOR, SECTORS_MAX};
use taaj::disk::{Disk, DiskError, Window};
use taaj::println;

taaj::program!(main);

/// What the boot test filled the disk with.
fn original(sector: u64, i: usize) -> u8 {
    (sector as usize).wrapping_mul(31).wrapping_add(i * 7).wrapping_add(i >> 8) as u8
}

/// What this program writes.
fn written(sector: u64, i: usize) -> u8 {
    ((sector as usize).wrapping_mul(13).wrapping_add(i * 3).wrapping_add(1) as u8) ^ 0xA5
}

fn holds(data: &[u8], sector: u64, pattern: fn(u64, usize) -> u8) -> bool {
    data.iter().enumerate().all(|(i, &byte)| byte == pattern(sector, i))
}

/// One sector written alone, then 32 written at once, right after it.
const WRITE_ONE: u64 = 100;
const WRITE_MANY: u64 = 101;
/// Where reads are checked against the original: never written.
const READ_RUN: u64 = 200;
const READ_RUN_LENGTH: u64 = 150;
const READ_MANY: u64 = 1000;
/// The smallest disk all of that fits on.
const SECTORS_NEEDED: u64 = 2048;

fn main() -> i64 {
    let (Some(writers), Some(readers)) = (taaj::startup_handle(0), taaj::startup_handle(1)) else {
        println!("[blkcheck] started without a disk");
        return 1;
    };
    let disk = Disk::new(writers);
    let look = Disk::new(readers);
    let mut checks = 0u32;

    macro_rules! check {
        ($cond:expr, $what:literal) => {
            if !$cond {
                println!("[blkcheck] FAILED: {}", $what);
                return line!() as i64;
            }
            checks += 1;
        };
    }

    // What the disk says of itself: the same on both channels.
    let Ok((sectors, read_only)) = disk.info() else {
        println!("[blkcheck] FAILED: the driver does not answer");
        return line!() as i64;
    };
    check!(sectors >= SECTORS_NEEDED, "the disk is too small for this test");
    check!(look.info() == Ok((sectors, read_only)), "the second channel describes the same disk");

    // Reading, one sector at a time: the first, the last, and a long
    // run - each a request, an interrupt and a reply.
    let mut one = [0u8; SECTOR];
    for sector in [0, 1, sectors - 2].into_iter().chain(READ_RUN..READ_RUN + READ_RUN_LENGTH) {
        one.fill(0xEE);
        check!(disk.read(sector, &mut one) == Ok(()), "a sector can be read");
        check!(holds(&one, sector, original), "a sector read holds what is on the disk there");
    }
    one.fill(0xEE);
    check!(look.read(7, &mut one) == Ok(()) && holds(&one, 7, original), "the second channel reads");

    // Reading many at once, through shared memory: all 32, and fewer -
    // what lies past the ones asked for must be left alone.
    let Ok(mut window) = Window::new(SECTORS_MAX) else {
        println!("[blkcheck] FAILED: no memory for a window");
        return line!() as i64;
    };
    let many = |window: &mut Window, first: u64, count: usize, pattern: fn(u64, usize) -> u8| {
        window.bytes().chunks(SECTOR).take(count).enumerate().all(|(n, sector)| holds(sector, first + n as u64, pattern))
    };
    window.bytes().fill(0xEE);
    check!(disk.read_many(&mut window, READ_MANY, SECTORS_MAX) == Ok(()), "32 sectors can be read at once");
    check!(many(&mut window, READ_MANY, SECTORS_MAX as usize, original), "32 sectors read at once are the right ones");
    window.bytes().fill(0xEE);
    check!(look.read_many(&mut window, READ_MANY + 5, 3) == Ok(()), "3 sectors can be read at once, on the second channel");
    check!(many(&mut window, READ_MANY + 5, 3, original), "3 sectors read at once are the right ones");
    check!(window.bytes()[3 * SECTOR..].iter().all(|&b| b == 0xEE), "a read of 3 sectors writes 3 sectors");

    // Past the end: refused, by number - the device is not asked.
    let last = sectors - 1;
    check!(disk.read(sectors, &mut one) == Err(DiskError::Refused(OUT_OF_RANGE)), "the sector after the last cannot be read");
    check!(disk.read(u64::MAX, &mut one) == Err(DiskError::Refused(OUT_OF_RANGE)), "sector 2^64-1 cannot be read");
    check!(
        disk.read_many(&mut window, last, 2) == Err(DiskError::Refused(OUT_OF_RANGE)),
        "a read that begins on the disk and ends past it is refused"
    );
    check!(disk.read_many(&mut window, last, 1) == Ok(()), "the last sector can be read through a window");
    check!(many(&mut window, last, 1, original) || !read_only, "and is the last sector");

    // Requests a careless client could send.
    let request = |op: u8, sector: u64, count: u32| {
        let mut bytes = [0u8; block::MANY_REQUEST_SIZE];
        bytes[0] = op;
        bytes[1..9].copy_from_slice(&sector.to_le_bytes());
        bytes[9..].copy_from_slice(&count.to_le_bytes());
        bytes
    };
    let lend = |memory: taaj::Handle| taaj::duplicate(memory, taaj::rights::ALL).ok();
    check!(disk.raw(&[], None) == Err(DiskError::Refused(BAD_REQUEST)), "an empty request is refused");
    check!(disk.raw(&[99], None) == Err(DiskError::Refused(BAD_REQUEST)), "an unknown request is refused");
    check!(disk.raw(&[block::READ, 0, 0], None) == Err(DiskError::Refused(BAD_REQUEST)), "a read cut short is refused");
    check!(
        disk.raw(&request(block::READ_MANY, 0, 1), None) == Err(DiskError::Refused(BAD_REQUEST)),
        "a read of many with no memory to put them in is refused"
    );
    if let Ok(small) = taaj::memory_create(4096) {
        // 8 sectors' worth of memory, 9 asked for; then 0, then 33.
        for (count, what_ok) in [(9, false), (0, false), (SECTORS_MAX + 1, false), (8, true)] {
            let lent = lend(small);
            check!(lent.is_some(), "a handle can be copied");
            let answer = disk.raw(&request(block::READ_MANY, 0, count), lent);
            check!(
                if what_ok { answer == Ok(()) } else { answer == Err(DiskError::Refused(BAD_REQUEST)) },
                "a read of many is checked against the memory lent, and against the limit"
            );
        }
        let _ = taaj::close(small);
    }
    // A handle that is no memory at all: one end of a channel.
    if let Ok((ours, theirs)) = taaj::channel_create() {
        check!(
            disk.raw(&request(block::READ_MANY, 0, 1), Some(theirs)) == Err(DiskError::Refused(BAD_REQUEST)),
            "a read of many into something that is not memory is refused"
        );
        let _ = taaj::close(ours);
    }
    // And after all that the driver still works.
    check!(disk.read(3, &mut one) == Ok(()) && holds(&one, 3, original), "the driver survives careless requests");

    // Writing. The second channel may never; the first, if the disk
    // allows it.
    let mut fresh = [0u8; SECTOR];
    for (i, byte) in fresh.iter_mut().enumerate() {
        *byte = written(WRITE_ONE, i);
    }
    check!(look.write(WRITE_ONE, &fresh) == Err(DiskError::Refused(READ_ONLY)), "the second channel cannot write");
    for (n, sector) in window.bytes().chunks_mut(SECTOR).enumerate() {
        for (i, byte) in sector.iter_mut().enumerate() {
            *byte = written(WRITE_MANY + n as u64, i);
        }
    }
    check!(
        look.write_many(&mut window, WRITE_MANY, SECTORS_MAX) == Err(DiskError::Refused(READ_ONLY)),
        "the second channel cannot write many either"
    );

    if read_only {
        check!(disk.write(WRITE_ONE, &fresh) == Err(DiskError::Refused(READ_ONLY)), "a read-only disk refuses a write");
        check!(
            disk.write_many(&mut window, WRITE_MANY, SECTORS_MAX) == Err(DiskError::Refused(READ_ONLY)),
            "a read-only disk refuses a write of many"
        );
        // And nothing was written, by anyone, ever.
        for sector in [WRITE_ONE, WRITE_MANY, WRITE_MANY + u64::from(SECTORS_MAX) - 1] {
            check!(disk.read(sector, &mut one) == Ok(()) && holds(&one, sector, original), "a read-only disk is unchanged");
        }
    } else {
        check!(disk.write(sectors, &fresh) == Err(DiskError::Refused(OUT_OF_RANGE)), "the sector after the last cannot be written");
        check!(
            disk.write_many(&mut window, last, 2) == Err(DiskError::Refused(OUT_OF_RANGE)),
            "a write that would end past the disk is refused, all of it"
        );

        check!(disk.write(WRITE_ONE, &fresh) == Ok(()), "a sector can be written");
        check!(disk.write_many(&mut window, WRITE_MANY, SECTORS_MAX) == Ok(()), "32 sectors can be written at once");

        // Read back the other way round: the many one at a time, the
        // one through the window - through the read-only channel, which
        // must see the same disk.
        for sector in [WRITE_MANY, WRITE_MANY + 13, WRITE_MANY + u64::from(SECTORS_MAX) - 1] {
            one.fill(0xEE);
            check!(look.read(sector, &mut one) == Ok(()) && holds(&one, sector, written), "what was written at once reads back");
        }
        window.bytes().fill(0xEE);
        check!(disk.read_many(&mut window, WRITE_ONE, SECTORS_MAX) == Ok(()), "the written sectors can be read at once");
        check!(many(&mut window, WRITE_ONE, SECTORS_MAX as usize, written), "everything written reads back");

        // The neighbours on both sides are as they were.
        for sector in [WRITE_ONE - 1, WRITE_MANY + u64::from(SECTORS_MAX)] {
            check!(disk.read(sector, &mut one) == Ok(()) && holds(&one, sector, original), "writing leaves the next sector alone");
        }

        // The very last sector: written and read back.
        for (i, byte) in fresh.iter_mut().enumerate() {
            *byte = written(last, i);
        }
        check!(disk.write(last, &fresh) == Ok(()), "the last sector can be written");
        one.fill(0xEE);
        check!(disk.read(last, &mut one) == Ok(()) && one == fresh, "the last sector reads back");
    }

    println!(
        "[blkcheck] {checks} checks, all passed: {sectors} sectors, {}",
        if read_only { "read-only - every write refused" } else { "33 written at 100 and the last one" }
    );
    0
}
