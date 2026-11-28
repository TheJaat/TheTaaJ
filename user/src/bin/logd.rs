//! logd: the system log service (K136). Reads the kernel's copy of the
//! debug log (`abi::log`, a ring init hands it), puts each writer's
//! pieces back into lines (the `syslog` crate), and keeps them in
//! /var/log/system.log on the disk: a line each, with the time, the writer
//! and a level - read with the `log` command.
//!
//! The file grows to `FILE_MAX` bytes; then it becomes system.log.1
//! (the one before is dropped) and a new one begins - the oldest lines
//! go first, and the log of the boots before this one stays.
//!
//! Startup handles: 1, the ring (to map, reading only); 2, the file
//! service. The serial line is not this service's: it has everything
//! already, whether this runs or not.

#![no_std]
#![no_main]

extern crate alloc;

use abi::log::{kind, Next, Reader, Ring};
use alloc::vec::Vec;
use syslog::{Level, Line, Lines, Source};
use taaj::files::Files;
use taaj::println;

taaj::program!(main);

const DIRECTORIES: [&str; 2] = ["/var", "/var/log"];
const FILE: &str = "/var/log/system.log";
const OLDER: &str = "/var/log/system.log.1";
/// The file's size before it is turned over.
const FILE_MAX: u64 = 1024 * 1024;
/// How often the ring is read, in ticks (100 a second).
const PERIOD: u64 = 20;
/// A line left open this long (a prompt) is written as it is.
const OPEN_FOR: u64 = 2_000_000_000;

fn main() -> i64 {
    let (Some(memory), Some(requests)) = (taaj::startup_handle(0), taaj::startup_handle(1)) else {
        println!("[logd] started without the ring and the file service");
        return 2;
    };
    let ring = match (taaj::memory_size(memory), taaj::memory_map(memory, false)) {
        // SAFETY: the kernel's ring, mapped for as long as this runs.
        (Ok(size), Ok(base)) => unsafe { Ring::open(base, size as usize) },
        _ => None,
    };
    let Some(ring) = ring else {
        println!("[logd] no system log ring from the kernel - nothing to keep");
        return 1;
    };
    let files = Files::new(requests);
    let mut reader = Reader::new(ring);
    let mut lines = Lines::new();
    let mut out: Vec<Line> = Vec::new();
    // The boots in the file, told apart.
    out.push(Line { time: taaj::realtime_ns(), source: "logd".into(), level: Level::Info, text: "--- the system started ---".into() });
    let mut kept_error = false;
    loop {
        loop {
            match reader.next() {
                Next::Record(record) => match record.kind {
                    kind::STARTED => lines.started(record.pid, core::str::from_utf8(record.name).unwrap_or("?")),
                    kind::KERNEL => lines.piece(Source::Kernel, record.time, record.text, &mut out),
                    kind::CRASH => lines.piece(Source::Crash, record.time, record.text, &mut out),
                    _ => lines.piece(Source::Program(record.pid), record.time, record.text, &mut out),
                },
                Next::Lost(bytes) => out.push(Line {
                    time: taaj::realtime_ns(),
                    source: "logd".into(),
                    level: Level::Warn,
                    text: alloc::format!("{bytes} bytes of the log are missing here: the ring filled before logd read it"),
                }),
                Next::Empty => break,
            }
        }
        lines.flush_older(OPEN_FOR, &mut out);
        if !out.is_empty() {
            let mut text = Vec::new();
            for line in out.drain(..) {
                text.extend_from_slice(syslog::format(&line).as_bytes());
            }
            match keep(&files, &text) {
                Ok(()) => kept_error = false,
                Err(error) => {
                    // Said once, on the serial line - not in a loop of
                    // its own lines.
                    if !kept_error {
                        println!("[logd] could not write {FILE}: {}", error.describe());
                    }
                    kept_error = true;
                }
            }
        }
        taaj::sleep(PERIOD);
    }
}

/// Appends `text` to the file, turning it over first if it is full;
/// the directory made first if it is not there yet - on a new disk, or
/// on one that was not there when logd started (fs serves files in
/// memory until the disk's driver answers).
fn keep(files: &Files, text: &[u8]) -> Result<(), taaj::files::FileError> {
    if files.size(FILE).is_ok_and(|size| size + text.len() as u64 > FILE_MAX) {
        let _ = files.remove(OLDER);
        files.rename(FILE, OLDER)?;
    }
    match files.append(FILE, text) {
        Ok(_) => Ok(()),
        Err(_) => {
            for directory in DIRECTORIES {
                let _ = files.make_directory(directory);
            }
            files.append(FILE, text).map(|_| ())
        }
    }
}
