//! write: makes a file hold one line of text, replacing what it held -
//! a program on the disk, not part of the shell. `write FILE TEXT`.
//!
//! The file is made if it is not there. The work is in common/store.rs,
//! shared by `write` and `append`.

#![no_std]
#![no_main]

extern crate alloc;

#[path = "common/store.rs"]
mod store;

taaj::program!(main);

fn main() -> i64 {
    store::store("write", false)
}
