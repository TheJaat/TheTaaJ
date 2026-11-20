//! yes: prints a line again and again, for ever - `y`, or its
//! arguments. A program on the disk, not part of the shell.
//!
//! It is the writer that never stops, which is what makes it useful:
//! `yes | given` shows that a pipeline ends when the reader does. `yes`
//! does not notice anything or decide anything - its next write finds
//! nobody at the other end, and the runtime ends the program there
//! (taaj::write). On the screen it stops with Ctrl+C.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::string::String;

use taaj::println;

taaj::program!(main);

fn main() -> i64 {
    let arguments = taaj::arguments();
    let line = if arguments.is_empty() { String::from("y") } else { arguments.join(" ") };
    loop {
        println!("{line}");
    }
}
