//! The user programs built into the kernel image.
//!
//! Until there is a file system to load them from, programs are compiled
//! first (the `user/` crate) and their ELF files embedded here. The
//! loader treats them exactly as it would a file from disk.

macro_rules! program {
    ($name:literal) => {
        include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../user/target/x86_64-unknown-none/release/", $name))
    };
}

pub static HELLO: &[u8] = program!("hello");
pub static PEEK: &[u8] = program!("peek");
pub static CLI: &[u8] = program!("cli");
pub static SPIN: &[u8] = program!("spin");
