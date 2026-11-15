//! given: says what it was started with.
//!
//! A program can reach only what it was handed. This one reports what
//! that is - so the shell's choices can be seen: run by its name it has
//! the screen and its arguments; run as `with files given`, the file
//! service as well.

#![no_std]
#![no_main]

use taaj::println;

taaj::program!(main);

fn main() -> i64 {
    let arguments = taaj::arguments();
    match taaj::file_service() {
        Some(files) => match files.list(|_, _| {}) {
            Ok(count) => println!("given: the screen, and the files ({count} of them)"),
            Err(error) => println!("given: the screen, and the files - but {}", error.describe()),
        },
        None => println!("given: the screen, no files"),
    }
    if !arguments.is_empty() {
        println!("given: arguments \"{}\"", arguments.trim());
    }
    0
}
