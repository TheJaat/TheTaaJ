//! trace: what the kernel does, for a while (K139) - system calls,
//! messages sent and received, threads switched, page faults,
//! interrupts - recorded on every CPU, then shown. A program on the
//! disk; the shell gives it tracing's handle (and the system's, to name
//! the processes).
//!
//!   trace            one second, then: the counts by kind, by CPU and
//!                    by process, and the last 20 events
//!   trace -s 5       five seconds
//!   trace -e 100     the last 100 events
//!
//! Tracing is on only while it records: off, each trace point costs the
//! kernel one load.

#![no_std]
#![no_main]

extern crate alloc;

use abi::trace::{kind, Event, Trace};
use abi::ProcessEntry;
use alloc::vec::Vec;
use taaj::{eprintln, println};

taaj::program!(main);

const KINDS: [u16; 7] = [kind::SYSCALL, kind::SEND, kind::RECEIVE, kind::SWITCH, kind::FAULT, kind::INTERRUPT, kind::SAMPLE];

/// The span's events, counted.
#[derive(Default)]
struct Counts {
    events: u64,
    by_kind: [u64; 7],
    by_cpu: Vec<u64>,
    by_pid: Vec<(u32, u64)>,
}

impl Counts {
    fn add(&mut self, event: &Event, cpus: usize) {
        self.events += 1;
        if let Some(n) = KINDS.iter().position(|&k| k == event.kind) {
            self.by_kind[n] += 1;
        }
        if self.by_cpu.len() < cpus {
            self.by_cpu.resize(cpus, 0);
        }
        if let Some(count) = self.by_cpu.get_mut(usize::from(event.cpu)) {
            *count += 1;
        }
        match self.by_pid.iter_mut().find(|(pid, _)| *pid == event.pid) {
            Some((_, count)) => *count += 1,
            None => self.by_pid.push((event.pid, 1)),
        }
    }
}

fn usage() -> i64 {
    eprintln!("usage: trace [-s SECONDS] [-e EVENTS]");
    2
}

fn main() -> i64 {
    let (mut seconds, mut shown) = (1u64, 20usize);
    let arguments = taaj::arguments();
    let mut words = arguments.iter();
    while let Some(word) = words.next() {
        let number = words.next().and_then(|n| n.parse::<u64>().ok());
        match (word.as_str(), number) {
            ("-s", Some(n)) if (1..=60).contains(&n) => seconds = n,
            ("-e", Some(n)) => shown = n as usize,
            _ => return usage(),
        }
    }
    let services = taaj::services();
    let Some(handle) = services.as_ref().and_then(|s| s.lookup("trace").ok()) else {
        eprintln!("trace: this program was not given tracing's handle");
        return 1;
    };
    let trace = match (taaj::memory_size(handle), taaj::memory_map(handle, true)) {
        // SAFETY: the kernel's rings, mapped for as long as this runs.
        (Ok(size), Ok(base)) => unsafe { Trace::open(base, size as usize) },
        _ => None,
    };
    let Some(trace) = trace else {
        eprintln!("trace: the handle is not tracing's rings");
        return 1;
    };
    if trace.on() {
        eprintln!("trace: tracing is on already - another trace is recording");
        return 1;
    }

    // The span: from where each CPU's ring is now, for `seconds`, read
    // every tenth of a second - a ring holds a second or two of a busy
    // CPU, a span may be longer. Counted as it goes; only the last
    // events are kept, to show.
    let mut from: Vec<u64> = (0..trace.cpus()).map(|cpu| trace.written(cpu)).collect();
    let mut counts = Counts::default();
    let mut last: alloc::collections::VecDeque<Event> = alloc::collections::VecDeque::new();
    let mut lost = 0;
    let mut batch: Vec<Event> = Vec::new();
    let start = taaj::monotonic_ns();
    trace.set_on(true);
    for tenth in 0..=seconds * 10 {
        if tenth == seconds * 10 {
            trace.set_on(false);
        } else {
            taaj::sleep(10);
        }
        batch.clear();
        lost += trace.read(&mut from, |event| batch.push(event));
        batch.sort_by_key(|event| event.time);
        for event in &batch {
            counts.add(event, trace.cpus());
            if last.len() == shown {
                last.pop_front();
            }
            if shown > 0 {
                last.push_back(*event);
            }
        }
    }
    let span = taaj::monotonic_ns() - start;

    // The processes' names, if the system's handle was given.
    let mut names = [ProcessEntry::default(); 32];
    let known = services
        .as_ref()
        .and_then(|s| s.lookup("system").ok())
        .and_then(|system| taaj::process_list(system, &mut names).ok())
        .map_or(0, |total| total.min(names.len()));
    let name = |pid: u32| -> &str {
        if pid == 0 {
            return "kernel";
        }
        names[..known].iter().find(|e| e.pid == u64::from(pid)).map_or("?", |e| e.name())
    };

    println!(
        "trace: {}.{:02} s on {} CPUs: {} events ({} lost)",
        span / 1_000_000_000,
        span / 10_000_000 % 100,
        trace.cpus(),
        counts.events,
        lost
    );
    let mut line = alloc::string::String::from("  by kind:");
    for (n, k) in KINDS.iter().enumerate() {
        line += &alloc::format!("  {} {}", kind::name(*k), counts.by_kind[n]);
    }
    println!("{line}");
    let mut line = alloc::string::String::from("  by CPU:");
    for (cpu, count) in counts.by_cpu.iter().enumerate().take(trace.cpus()) {
        line += &alloc::format!("  cpu{cpu} {count}");
    }
    println!("{line}");
    // The busiest processes.
    counts.by_pid.sort_by(|a, b| b.1.cmp(&a.1));
    println!("  by process:");
    for (pid, count) in counts.by_pid.iter().take(8) {
        println!("    {:>5} {:<12} {}", pid, name(*pid), count);
    }
    println!("  the last {} events (ms from the start, CPU, process, what, value):", last.len());
    for event in &last {
        let at = event.time.saturating_sub(start);
        println!(
            "    {:>6}.{:03}  cpu{}  {:>5} {:<12} {:<9} {:#x}",
            at / 1_000_000,
            at / 1_000 % 1000,
            event.cpu,
            event.pid,
            name(event.pid),
            kind::name(event.kind),
            event.value
        );
    }
    0
}
