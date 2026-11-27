//! dev - /dev's service (J65): null, zero, full, random and urandom, as
//! files any program given it can open, read and write.
//!
//! Startup handles, from init:
//!   1  the request channel (receive only): the file protocol, as fs
//!      speaks it. There is no second, read-only channel: nothing here
//!      can be changed, made or removed, whoever asks, so there is
//!      nothing for one to protect. A program that may only read the
//!      disk's files may still write to /dev/null.
//!
//! The devices are the `fileserver` crate's DevStore, tested on the
//! host; the request loop is fs's (common/serve.rs). This program only
//! gives the store the kernel's random generator.
//!
//! Nothing here is kept: a restart changes nothing a program could see.

#![no_std]
#![no_main]

extern crate alloc;

use fileserver::DevStore;
use taaj::println;

// The request loop, shared with fs.
#[path = "common/serve.rs"]
mod serve;

taaj::program!(main);

/// Random bytes from the kernel's generator (J2): false if it refused.
fn random(out: &mut [u8]) -> bool {
    taaj::random(out).is_ok()
}

fn main() -> i64 {
    let Some(requests) = taaj::startup_handle(0) else {
        println!("[dev] started without a request channel");
        return 1;
    };
    let mut names = alloc::string::String::new();
    for name in DevStore::names() {
        names.push(' ');
        names.push_str(name);
    }
    println!("[dev] ready:{names}");
    serve::serve("dev", &mut DevStore::new(random), requests, None)
}
