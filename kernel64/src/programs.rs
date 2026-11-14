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
pub static ECHOD: &[u8] = program!("echod");
pub static ECHOC: &[u8] = program!("echoc");
pub static PORT: &[u8] = program!("port");
pub static KBD: &[u8] = program!("kbd");
pub static SHELL: &[u8] = program!("shell");
pub static INIT: &[u8] = program!("init");
pub static DISPLAY: &[u8] = program!("display");
pub static MEMRO: &[u8] = program!("memro");

/// The programs a launcher can start, by name.
static PROGRAMS: &[(&str, &[u8])] = &[
    ("kbd", KBD),
    ("shell", SHELL),
    ("display", DISPLAY),
    ("echod", ECHOD),
    ("echoc", ECHOC),
    ("hello", HELLO),
];

pub fn find(name: &str) -> Option<(&'static str, &'static [u8])> {
    PROGRAMS.iter().find(|(n, _)| *n == name).copied()
}
