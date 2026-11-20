//! cp: copies a file - a program on the disk, not part of the shell.
//! `cp FROM TO`.
//!
//! If TO is a directory, the copy goes into it under FROM's name;
//! otherwise TO is the copy's path. A directory is not copied (there is
//! no copying of trees yet), and nothing is replaced: if TO names
//! something, cp says so and stops.
//!
//! Unlike mv, this is not one step. The file service has no "copy": cp
//! reads the file a message's worth at a time and appends each piece to
//! the new one, and every append is its own change on the disk. If it
//! fails on the way - the disk fills up - cp removes the part it made,
//! so what is left is the copy or nothing; but a power cut in the
//! middle leaves a short copy behind.

#![no_std]
#![no_main]

extern crate alloc;

#[path = "common/target.rs"]
mod target;

use fileserver::protocol::Status;
use taaj::files::{FileError, Kind};
use taaj::println;

taaj::program!(main);

fn main() -> i64 {
    let arguments = taaj::arguments();
    let Some(files) = taaj::file_service() else {
        println!("cp: started without the file service");
        return 2;
    };
    let mut words = arguments.split_whitespace();
    let (Some(from), Some(to), None) = (words.next(), words.next(), words.next()) else {
        println!("usage: cp FROM TO");
        return 2;
    };
    match files.stat(from) {
        Ok((Kind::File, _)) => {}
        Ok((Kind::Directory, _)) => {
            println!("cp: {from}: is a directory");
            return 1;
        }
        Err(error) => {
            println!("cp: {from}: {}", error.describe());
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
            println!("cp: {to}: {}", Status::Exists.describe());
            return 1;
        }
        Err(error) => {
            println!("cp: {to}: {}", error.describe());
            return 1;
        }
    }
    // An empty file first - so an empty file can be copied at all, and
    // so a bad path is found out before anything is read.
    let mut copied = files.write(&to, &[]);
    if let Err(error) = copied {
        println!("cp: {to}: {}", error.describe());
        return 1;
    }
    // `read` hands over the file piece by piece; each piece is appended
    // as it comes, so no more than one message is ever held in memory.
    // After a failed append the rest is still read, and dropped.
    let read = files.read(from, |piece| {
        if copied.is_ok() {
            copied = files.append(&to, piece);
        }
    });
    match (read, copied) {
        (Ok(_), Ok(size)) => {
            println!("{to}: {size} bytes");
            0
        }
        (read, copied) => {
            // Which of the two failed says whose name goes in the message.
            match (read, copied) {
                (Err(error), _) => println!("cp: {from}: {}", error.describe()),
                (_, Err(error)) => println!("cp: {to}: {}", error.describe()),
                _ => {}
            }
            if files.remove(&to).is_err() {
                println!("cp: {to}: is incomplete, and could not be removed");
            }
            1
        }
    }
}
