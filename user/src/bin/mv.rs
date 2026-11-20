//! mv: moves or renames a file or a directory - a program on the disk,
//! not part of the shell. `mv FROM TO`.
//!
//! If TO is a directory, FROM goes into it and keeps its name;
//! otherwise TO is its new path. Nothing is copied: the file service
//! gives the same blocks another name, in one step that happens wholly
//! or not at all - so a directory moves, with everything in it, as fast
//! as a file does. And nothing is replaced: if TO names a file, mv
//! says so and stops.

#![no_std]
#![no_main]

extern crate alloc;

#[path = "common/target.rs"]
mod target;

use taaj::println;

taaj::program!(main);

fn main() -> i64 {
    let arguments = taaj::arguments();
    let Some(files) = taaj::file_service() else {
        println!("mv: started without the file service");
        return 2;
    };
    let mut words = arguments.split_whitespace();
    let (Some(from), Some(to), None) = (words.next(), words.next(), words.next()) else {
        println!("usage: mv FROM TO");
        return 2;
    };
    let to = target::target(&files, from, to);
    match files.rename(from, &to) {
        Ok(()) => 0,
        Err(error) => {
            println!("mv: {from} to {to}: {}", error.describe());
            1
        }
    }
}
