//! top: the processes, and what each is using - CPU, memory, threads,
//! handles - refreshed (K141). A program on the disk; the shell gives
//! it the system's handle to look with, as `ps` has.
//!
//!   top           every second, until Ctrl+C
//!   top -n 3      three times, then ends
//!   top -d 5      every five seconds
//!
//! CPU% is of one CPU, over the last interval: a program that keeps two
//! CPUs busy shows 200%. The busiest first; ended processes (still
//! referenced) are left out.

#![no_std]
#![no_main]

extern crate alloc;

use abi::ProcessEntry;
use alloc::vec::Vec;
use taaj::{eprintln, print, println};

taaj::program!(main);

const MIB: u64 = 1 << 20;

fn usage() -> i64 {
    eprintln!("usage: top [-n TIMES] [-d SECONDS]");
    2
}

/// The running processes now: their entries, and how many exist.
fn sample(system: taaj::Handle) -> Option<(Vec<ProcessEntry>, usize)> {
    let mut entries = [ProcessEntry::default(); 32];
    let total = taaj::process_list(system, &mut entries).ok()?;
    Some((entries[..total.min(entries.len())].iter().filter(|e| e.exit_code.is_none()).copied().collect(), total))
}

fn main() -> i64 {
    let (mut times, mut seconds) = (None::<u64>, 1u64);
    let arguments = taaj::arguments();
    let mut words = arguments.iter();
    while let Some(word) = words.next() {
        let number = words.next().and_then(|n| n.parse::<u64>().ok()).filter(|&n| n > 0);
        match (word.as_str(), number) {
            ("-n", Some(n)) => times = Some(n),
            ("-d", Some(n)) => seconds = n,
            _ => return usage(),
        }
    }
    let Some(system) = taaj::services().and_then(|services| services.lookup("system").ok()) else {
        eprintln!("top: this program was not given the system's handle");
        return 1;
    };
    let Some((mut before, _)) = sample(system) else {
        eprintln!("top: the processes cannot be listed");
        return 1;
    };
    let mut then = taaj::monotonic_ns();
    let mut shown = 0;
    while times.is_none_or(|n| shown < n) {
        taaj::sleep(seconds * 100);
        let Some((now_entries, total)) = sample(system) else { return 1 };
        let now = taaj::monotonic_ns();
        let elapsed = now.saturating_sub(then).max(1);
        // CPU in tenths of a percent of one CPU, over the interval.
        let mut rows: Vec<(u64, ProcessEntry)> = now_entries
            .iter()
            .map(|entry| {
                let earlier = before.iter().find(|b| b.pid == entry.pid).map_or(0, |b| b.cpu_ns);
                (entry.cpu_ns.saturating_sub(earlier).saturating_mul(1000) / elapsed, *entry)
            })
            .collect();
        rows.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.memory.cmp(&a.1.memory)));
        // A clear screen first (as `clear`), then the frame.
        print!("\x0c");
        match taaj::system_info(system) {
            Ok(info) => println!(
                "top: {} processes, {} threads, {} channels; memory {} MiB used of {}; {} CPUs",
                info.processes,
                info.threads,
                info.channels,
                (info.frames_total - info.frames_free) * 4096 / MIB,
                info.frames_total * 4096 / MIB,
                info.cpus
            ),
            Err(_) => println!("top: {total} processes"),
        }
        println!("  PID  NAME            CPU%      CPU TIME    MEMORY  THREADS  HANDLES");
        for (tenths, entry) in &rows {
            let millis = entry.cpu_ns / 1_000_000;
            println!(
                "{:>5}  {:<12} {:>4}.{}%  {:>6}.{:03} s  {:>6} KiB  {:>7}  {:>7}",
                entry.pid,
                entry.name(),
                tenths / 10,
                tenths % 10,
                millis / 1000,
                millis % 1000,
                entry.memory / 1024,
                entry.threads,
                entry.handles
            );
        }
        if total > 32 {
            println!("({} more processes not shown)", total - 32);
        }
        before = now_entries;
        then = now;
        shown += 1;
    }
    0
}
