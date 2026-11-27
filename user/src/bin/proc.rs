//! proc - /proc's service (J66): what the kernel says about the machine
//! and its processes, as files any program given it can read.
//!
//! Startup handles, from init:
//!   1  the request channel (receive only): the file protocol, as fs
//!      speaks it. One channel: nothing here can be changed, whoever
//!      asks.
//!   2  the system handle: with it the kernel answers system_info and
//!      process_list - looking, never touching (the shell's `ps`, `mem`
//!      and `uptime` hold the same).
//!
//! The files are the `fileserver` crate's ProcStore, tested on the host
//! with made-up numbers; the request loop is fs's (common/serve.rs).
//! This program only asks the kernel.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec::Vec;

use abi::{ProcessEntry, SystemInfo};
use fileserver::proc::Source;
use fileserver::ProcStore;
use taaj::{println, Handle};

// The request loop, shared with fs and dev.
#[path = "common/serve.rs"]
mod serve;

taaj::program!(main);

/// The kernel, asked through the system handle.
struct Kernel(Handle);

impl Source for Kernel {
    fn system(&mut self) -> Option<SystemInfo> {
        taaj::system_info(self.0).ok()
    }

    fn processes(&mut self) -> Vec<ProcessEntry> {
        // As many as process_list gives in one call (32); a machine with
        // more shows its first 32.
        let mut entries = [ProcessEntry::default(); 32];
        match taaj::process_list(self.0, &mut entries) {
            Ok(total) => entries[..total.min(entries.len())].to_vec(),
            Err(_) => Vec::new(),
        }
    }
}

fn main() -> i64 {
    let Some(requests) = taaj::startup_handle(0) else {
        println!("[proc] started without a request channel");
        return 1;
    };
    let Some(system) = taaj::startup_handle(1) else {
        println!("[proc] started without the system handle");
        return 1;
    };
    println!("[proc] ready: uptime meminfo cpuinfo stat, and PID/status for each process");
    serve::serve("proc", &mut ProcStore::new(Kernel(system)), requests, None)
}
