//! log: the system log (K136), as the log service keeps it in
//! /var/log/system.log (and system.log.1 before it) - a program on the disk.
//!
//!   log                 the last 20 lines
//!   log -n 100          the last 100
//!   log -a              all of it, the oldest first
//!   log -l warn         only warn and worse (info, warn, error, crit)
//!   log -s ahci         only what ahci wrote ("kernel" for the kernel's)
//!   log -b              only since the system last started
//!
//! The options go together: `log -l error -n 5`.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use syslog::Level;
use taaj::{eprintln, print};

taaj::program!(main);

const FILES: [&str; 2] = ["/var/log/system.log.1", "/var/log/system.log"];
const STARTED: &str = "--- the system started ---";

fn usage() -> i64 {
    eprintln!("usage: log [-n LINES | -a] [-l info|warn|error|crit] [-s SOURCE] [-b]");
    2
}

fn main() -> i64 {
    let (mut last, mut level, mut source, mut boot) = (Some(20usize), Level::Info, None::<String>, false);
    let arguments = taaj::arguments();
    let mut words = arguments.iter();
    while let Some(word) = words.next() {
        match word.as_str() {
            "-a" => last = None,
            "-b" => boot = true,
            "-n" => match words.next().and_then(|n| n.parse().ok()) {
                Some(n) => last = Some(n),
                None => return usage(),
            },
            "-l" => match words.next().and_then(|name| Level::parse(name)) {
                Some(chosen) => level = chosen,
                None => return usage(),
            },
            "-s" => match words.next() {
                Some(name) => source = Some(name.clone()),
                None => return usage(),
            },
            _ => return usage(),
        }
    }
    let Some(files) = taaj::file_service() else {
        eprintln!("log: no file service for this program");
        return 1;
    };
    let mut text = Vec::new();
    let mut found = false;
    for name in FILES {
        if files.read(name, |piece| text.extend_from_slice(piece)).is_ok() {
            found = true;
        }
    }
    if !found {
        eprintln!("log: no system log yet ({} not there: no disk, or the log service has not written)", FILES[1]);
        return 1;
    }
    let text = String::from_utf8_lossy(&text);
    let mut chosen: Vec<&str> = Vec::new();
    for line in text.split_inclusive('\n') {
        let Some((_, from, its_level, said)) = syslog::parse(line) else { continue };
        if boot && from == "logd" && said == STARTED {
            chosen.clear();
        }
        if its_level >= level && source.as_deref().is_none_or(|wanted| wanted == from) {
            chosen.push(line);
        }
    }
    let skip = last.map_or(0, |n| chosen.len().saturating_sub(n));
    for line in &chosen[skip..] {
        print!("{line}");
    }
    0
}
