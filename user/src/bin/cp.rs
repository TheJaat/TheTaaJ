//! cp: copies a file - a program on the disk, not part of the shell.
//! `cp FROM TO`.
//!
//! If TO is a directory, the copy goes into it under FROM's name;
//! otherwise TO is the copy's path. A directory is not copied (there is
//! no copying of trees yet), and nothing is replaced: if TO names
//! something, cp says so and stops.
//!
//! Unlike mv, this is not one step. The file service has no "copy": cp
//! reads the file a message's worth at a time, gathers the pieces, and
//! appends them a megabyte (protocol::WRITE_FROM_MAX) at a time - each
//! append its own change on the disk, a few for a big file. If it
//! fails on the way - the disk fills up - cp removes the part it made,
//! so what is left is the copy or nothing; but a power cut in the
//! middle leaves a short copy behind.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec::Vec;

#[path = "common/target.rs"]
mod target;

use fileserver::protocol::{self, Status};
use taaj::files::{FileError, Kind};
use taaj::{eprintln, println};

taaj::program!(main);

fn main() -> i64 {
    let arguments = taaj::arguments();
    let Some(files) = taaj::file_service() else {
        eprintln!("cp: started without the file service");
        return 2;
    };
    // A slice pattern: exactly two arguments, and names for both.
    let [from, to] = &arguments[..] else {
        eprintln!("usage: cp FROM TO");
        return 2;
    };
    match files.stat(from) {
        Ok((Kind::File, _)) => {}
        Ok((Kind::Directory, _)) => {
            eprintln!("cp: {from}: is a directory");
            return 1;
        }
        Err(error) => {
            eprintln!("cp: {from}: {}", error.describe());
            return 1;
        }
    }
    let to = target::target(&files, from, to);
    // Free, or cp stops: looked at first, because writing is how a file
    // is replaced. (Looked at and then written are two requests: a
    // program that made the file in between would have it overwritten.
    // The file service would need a "make, only if new" to close that.)
    match files.stat(&to) {
        Err(FileError::Refused(Status::NotFound)) => {}
        Ok(_) => {
            eprintln!("cp: {to}: {}", Status::Exists.describe());
            return 1;
        }
        Err(error) => {
            eprintln!("cp: {to}: {}", error.describe());
            return 1;
        }
    }
    // An empty file first - so an empty file can be copied at all, and
    // so a bad path is found out before anything is read.
    let mut copied = files.write(&to, &[]);
    if let Err(error) = copied {
        eprintln!("cp: {to}: {}", error.describe());
        return 1;
    }
    // `read` hands over the file piece by piece; the pieces are gathered
    // and appended when a megabyte is held, and at the end: a commit a
    // megabyte, not one a piece. After a failed append the rest is
    // still read, and dropped.
    let mut held: Vec<u8> = Vec::new();
    let read = files.read(from, |piece| {
        if copied.is_ok() {
            held.extend_from_slice(piece);
            if held.len() >= protocol::WRITE_FROM_MAX {
                copied = files.append(&to, &held);
                held.clear();
            }
        }
    });
    if copied.is_ok() && !held.is_empty() {
        copied = files.append(&to, &held);
    }
    match (read, copied) {
        (Ok(_), Ok(size)) => {
            println!("{to}: {size} bytes");
            0
        }
        (read, copied) => {
            // Which of the two failed says whose name goes in the message.
            match (read, copied) {
                (Err(error), _) => eprintln!("cp: {from}: {}", error.describe()),
                (_, Err(error)) => eprintln!("cp: {to}: {}", error.describe()),
                _ => {}
            }
            if files.remove(&to).is_err() {
                eprintln!("cp: {to}: is incomplete, and could not be removed");
            }
            1
        }
    }
}
