//! profile: where the CPUs spend their time (K140). The timer samples
//! where each CPU is, a hundred times a second or more (tracing's
//! `sample` events, K139); this counts them for a while and says where
//! the time went - each sample standing for the time since its CPU's
//! last one, as the timer comes more often to a CPU that sleeps (its
//! wake-ups) than the tick alone would: counted plainly, idle would
//! look busier than it is. Where it went:
//! the kernel's functions by name (the kernel's symbols come with
//! tracing's handle), programs by name and address (programs are built
//! without their symbols).
//!
//!   profile          five seconds, the 15 busiest places
//!   profile -s 10    ten seconds
//!   profile -n 30    the 30 busiest
//!
//! A program on the disk; the shell gives it tracing's handle, the
//! kernel's symbols and the system's (to name the processes).

#![no_std]
#![no_main]

extern crate alloc;

use abi::symbols::Table;
use abi::trace::{kind, Trace};
use abi::ProcessEntry;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use taaj::{eprintln, println};

taaj::program!(main);

/// Addresses from here up are the kernel's (both CPUs' kernels live in
/// the top half).
const KERNEL: u64 = 0xffff_8000_0000_0000;

fn usage() -> i64 {
    eprintln!("usage: profile [-s SECONDS] [-n PLACES]");
    2
}

/// A place the time went: the kernel in one of its functions, or a
/// process at an address.
#[derive(PartialEq, Eq, Clone)]
enum Place {
    Kernel(String),
    Program(u32, u64),
}

fn main() -> i64 {
    let (mut seconds, mut places) = (5u64, 15usize);
    let arguments = taaj::arguments();
    let mut words = arguments.iter();
    while let Some(word) = words.next() {
        let number = words.next().and_then(|n| n.parse::<u64>().ok());
        match (word.as_str(), number) {
            ("-s", Some(n)) if (1..=60).contains(&n) => seconds = n,
            ("-n", Some(n)) if n > 0 => places = n as usize,
            _ => return usage(),
        }
    }
    let services = taaj::services();
    let lookup = |name: &str| services.as_ref().and_then(|s| s.lookup(name).ok());
    let Some(handle) = lookup("trace") else {
        eprintln!("profile: this program was not given tracing's handle");
        return 1;
    };
    let trace = match (taaj::memory_size(handle), taaj::memory_map(handle, true)) {
        // SAFETY: the kernel's rings, mapped for as long as this runs.
        (Ok(size), Ok(base)) => unsafe { Trace::open(base, size as usize) },
        _ => None,
    };
    let Some(trace) = trace else {
        eprintln!("profile: the handle is not tracing's rings");
        return 1;
    };
    let symbols: Option<&[u8]> = lookup("trace.symbols").and_then(|h| match (taaj::memory_size(h), taaj::memory_map(h, false)) {
        // SAFETY: the kernel's symbols, `size` bytes, mapped for as long
        // as this runs, and only read.
        (Ok(size), Ok(base)) => Some(unsafe { core::slice::from_raw_parts(base, size as usize) }),
        _ => None,
    });
    let table = symbols.and_then(Table::parse);
    if trace.on() {
        eprintln!("profile: tracing is on already - a trace or a profile is recording");
        return 1;
    }

    // Record, reading every tenth of a second (as `trace` does); only
    // the samples count.
    let mut from: Vec<u64> = (0..trace.cpus()).map(|cpu| trace.written(cpu)).collect();
    let mut counted: Vec<(Place, u64)> = Vec::new();
    let mut by_program: Vec<(u32, u64)> = Vec::new();
    let (mut samples, mut lost, mut total) = (0u64, 0u64, 0u64);
    // Each CPU's last sample: the time each sample stands for.
    let mut last: Vec<Option<u64>> = (0..trace.cpus()).map(|_| None).collect();
    let start = taaj::monotonic_ns();
    trace.set_on(true);
    for tenth in 0..=seconds * 10 {
        if tenth == seconds * 10 {
            trace.set_on(false);
        } else {
            taaj::sleep(10);
        }
        lost += trace.read(&mut from, |event| {
            if event.kind != kind::SAMPLE {
                return;
            }
            samples += 1;
            // The time since this CPU's last sample - a tick (10 ms) for
            // the first, and at most two: a CPU asleep for long was not
            // anywhere in particular.
            let previous = last.get_mut(usize::from(event.cpu)).and_then(|l| l.replace(event.time));
            let weight = previous.map_or(10_000_000, |p| event.time.saturating_sub(p).min(20_000_000));
            total += weight;
            let (place, pid) = if event.value >= KERNEL {
                let name = table.as_ref().and_then(|t| t.lookup(event.value)).map_or_else(|| format!("{:#x}", event.value), |(n, _)| String::from(n));
                (Place::Kernel(name), 0)
            } else {
                (Place::Program(event.pid, event.value), event.pid)
            };
            match counted.iter_mut().find(|(p, _)| *p == place) {
                Some((_, n)) => *n += weight,
                None => counted.push((place, weight)),
            }
            match by_program.iter_mut().find(|(p, _)| *p == pid) {
                Some((_, n)) => *n += weight,
                None => by_program.push((pid, weight)),
            }
        });
    }
    let span = taaj::monotonic_ns() - start;

    let mut names = [ProcessEntry::default(); 32];
    let known = lookup("system").and_then(|system| taaj::process_list(system, &mut names).ok()).map_or(0, |t| t.min(names.len()));
    let name = |pid: u32| -> &str {
        if pid == 0 {
            return "kernel";
        }
        names[..known].iter().find(|e| e.pid == u64::from(pid)).map_or("?", |e| e.name())
    };
    // Of all the time the samples stand for; and that time in ms.
    let percent = |n: u64| {
        let tenths = n * 1000 / total.max(1);
        format!("{:>3}.{}%  {:>6} ms", tenths / 10, tenths % 10, n / 1_000_000)
    };

    println!(
        "profile: {}.{:02} s on {} CPUs: {} samples ({} lost){}",
        span / 1_000_000_000,
        span / 10_000_000 % 100,
        trace.cpus(),
        samples,
        lost,
        if table.is_some() { "" } else { " - no kernel symbols: the kernel's places are addresses" }
    );
    by_program.sort_by(|a, b| b.1.cmp(&a.1));
    println!("  by program:");
    for (pid, n) in by_program.iter().take(8) {
        println!("    {}  {:>5} {}", percent(*n), pid, name(*pid));
    }
    counted.sort_by(|a, b| b.1.cmp(&a.1));
    println!("  the {} busiest places:", counted.len().min(places));
    for (place, n) in counted.iter().take(places) {
        match place {
            Place::Kernel(function) => println!("    {}  kernel  {}", percent(*n), function),
            Place::Program(pid, address) => println!("    {}  {} ({})  at {:#x}", percent(*n), name(*pid), pid, address),
        }
    }
    0
}
