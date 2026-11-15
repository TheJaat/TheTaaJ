//! mkdir: makes directories - a program on the disk, not part of the
//! shell.
//!
//! `mkdir DIR...`. Each is made in a directory that must be there
//! already: `mkdir a/b` needs `a`. It exits with 1 if any of them could
//! not be made.

#![no_std]
#![no_main]

use taaj::println;

taaj::program!(main);

fn main() -> i64 {
    let arguments = taaj::arguments();
    let Some(files) = taaj::file_service() else {
        println!("mkdir: started without the file service");
        return 2;
    };
    if arguments.trim().is_empty() {
        println!("usage: mkdir DIR...");
        return 2;
    }
    let mut failed = false;
    for path in arguments.split_whitespace() {
        if let Err(error) = files.make_directory(path) {
            println!("mkdir: {path}: {}", error.describe());
            failed = true;
        }
    }
    i64::from(failed)
}
