//! killcheck: the files half of the test that kills a disk driver in
//! the middle of its I/O (K82).
//!
//!   killcheck write SET COUNT   writes files SET0 .. SET(COUNT-1)
//!   killcheck check SET COUNT   reads them back
//!
//! Each file is 8 KiB, its bytes made from its name and its number, so
//! that a file holding another's bytes, or half of the old and half of
//! the new, is told from the right one. A write the file service
//! answered is on the disk: TaajFS commits every change before it
//! answers (copy-on-write, the disk's cache flushed). So each answered
//! write is said - "killcheck: acked SET3" - and the test checks every
//! acked file later, here and on the disk image after the machine is
//! off.
//!
//! The driver dies under some write (`disk crash N`): the file service
//! is told its request failed, and passes that on. A write that fails
//! is said, and tried again a second later, up to 20 times: init starts
//! a new driver at once. A file never acked is not checked - it may
//! hold anything, or not be there - but every acked one must be whole.
//!
//! Exit code: 0 if every file was written (write) or as written
//! (check); 1 if not; 2 for a wrong command line.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use taaj::{eprintln, println};

taaj::program!(main);

const SIZE: usize = 8192;
const TRIES: u32 = 20;

/// File `name`'s bytes: its name on the first line, then a pattern of
/// its number - different in every file, and at every offset.
fn contents(name: &str, number: u32) -> Vec<u8> {
    let mut data = Vec::with_capacity(SIZE);
    data.extend_from_slice(format!("killcheck file {name}\n").as_bytes());
    let seed = number.wrapping_mul(2_654_435_761);
    while data.len() < SIZE {
        let at = data.len() as u32;
        data.push((seed.wrapping_add(at.wrapping_mul(7)) >> 3) as u8);
    }
    data
}

fn main() -> i64 {
    let arguments = taaj::arguments();
    let Some(files) = taaj::file_service() else {
        eprintln!("killcheck: started without the file service");
        return 2;
    };
    let words: Vec<&str> = arguments.iter().map(String::as_str).collect();
    let (action, set, count) = match words.as_slice() {
        [action @ ("write" | "check"), set, count] => match count.parse::<u32>() {
            Ok(count @ 1..=1000) => (*action, *set, count),
            _ => return usage(),
        },
        _ => return usage(),
    };
    if action == "write" {
        let (mut acked, mut retried) = (0, 0);
        for number in 0..count {
            let name = format!("{set}{number}");
            let data = contents(&name, number);
            let mut tries = 0;
            loop {
                tries += 1;
                match files.write(&name, &data) {
                    Ok(_) => {
                        println!("killcheck: acked {name}");
                        acked += 1;
                        break;
                    }
                    Err(error) if tries < TRIES => {
                        println!("killcheck: {name}: {} - trying again", error.describe());
                        retried += 1;
                        taaj::sleep(100);
                    }
                    Err(error) => {
                        println!("killcheck: {name}: {} - given up after {TRIES} tries", error.describe());
                        println!("killcheck: wrote {acked} of {count}, {retried} tries again");
                        return 1;
                    }
                }
            }
        }
        println!("killcheck: wrote {acked} of {count} files of {SIZE} bytes, {retried} tries again");
        return 0;
    }
    let mut wrong = 0;
    for number in 0..count {
        let name = format!("{set}{number}");
        let want = contents(&name, number);
        let mut got = Vec::with_capacity(SIZE);
        match files.read(&name, |piece| got.extend_from_slice(piece)) {
            Ok(_) if got == want => {}
            Ok(_) => {
                let at = got.iter().zip(&want).position(|(a, b)| a != b).unwrap_or(got.len().min(want.len()));
                println!("killcheck: {name} is not as written: {} bytes, the first difference at {at}", got.len());
                wrong += 1;
            }
            Err(error) => {
                println!("killcheck: {name}: {}", error.describe());
                wrong += 1;
            }
        }
    }
    if wrong == 0 {
        println!("killcheck: {count} files of {set}, all as written");
        0
    } else {
        println!("killcheck: {wrong} of {count} files of {set} not as written");
        1
    }
}

fn usage() -> i64 {
    eprintln!("usage: killcheck write SET COUNT, killcheck check SET COUNT (COUNT 1 to 1000)");
    2
}
