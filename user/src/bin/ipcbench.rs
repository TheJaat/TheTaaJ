//! ipcbench: the IPC benchmark (K60) - what a message costs, measured
//! from a program, so that the `ipc` lane's work (K61 `channel_call`,
//! K62 one copy per message) has numbers to be measured against.
//!
//!   with programs ipcbench all      every measurement
//!   ipcbench serve                  (the other end, started by `all`)
//!
//! Measured:
//!   - the round trip of a small request (16 bytes) and its reply, to
//!     another program (a switch of address space each way) and to
//!     another thread of this one;
//!   - bulk data through messages of 1024 bytes, one way;
//!   - bulk data through a shared memory object: written here, read
//!     there, a short message each way per megabyte.
//!
//! The other program is started through init with two channels as its
//! input and output (taaj::start): requests come in on one, replies go
//! out on the other. Only the numbers are printed: whether they are
//! good is for the journal, not for a test - an emulated CPU or a busy
//! host is slower, and that is not a failure.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec::Vec;
use taaj::{eprintln, println, Error, Handle};

taaj::program!(main);

const ROUND_TRIPS: usize = 5000;
const BULK_BYTES: usize = 8 << 20;
const MESSAGE: usize = 1024;
const SHARED: u64 = 1 << 20;
const SHARED_ROUNDS: u64 = 16;

// The requests: a kind byte first.
const ROUND_TRIP: u8 = b'R';
const BULK: u8 = b'B';
const BULK_END: u8 = b'E';
const MEMORY: u8 = b'M';
const GO: u8 = b'G';
const QUIT: u8 = b'Q';

/// The other end: answers requests on `requests`, replies on `replies`,
/// until QUIT (or the channel closes).
fn serve(requests: Handle, replies: Handle) {
    let mut message = [0u8; MESSAGE];
    let mut handles = [0u64; 1];
    let mut bulk = 0u64;
    let mut shared: Option<*const u8> = None;
    loop {
        let Ok((length, count)) = taaj::receive(requests, &mut message, &mut handles) else { return };
        match message.first().copied() {
            Some(ROUND_TRIP) => {
                let _ = send(replies, &message[..length], &[]);
            }
            Some(BULK) => bulk += length as u64,
            Some(BULK_END) => {
                let _ = send(replies, &bulk.to_le_bytes(), &[]);
                bulk = 0;
            }
            Some(MEMORY) if count == 1 => {
                shared = taaj::memory_map(handles[0], false).ok().map(|p| p as *const u8);
                let _ = send(replies, b"ok", &[]);
            }
            Some(GO) => {
                let sum = shared.map_or(0u64, |base| {
                    // SAFETY: SHARED bytes, mapped above for as long as
                    // this runs.
                    let bytes = unsafe { core::slice::from_raw_parts(base, SHARED as usize) };
                    bytes.chunks_exact(8).fold(0u64, |s, w| s.wrapping_add(u64::from_le_bytes(w.try_into().unwrap_or([0; 8]))))
                });
                let _ = send(replies, &sum.to_le_bytes(), &[]);
            }
            Some(QUIT) | None => return,
            _ => {}
        }
    }
}

/// Sends, waiting while the queue is full: bulk data outruns its reader.
fn send(channel: Handle, data: &[u8], handles: &[Handle]) -> Result<(), Error> {
    loop {
        match taaj::send(channel, data, handles) {
            Err(Error::QueueFull) => taaj::yield_now(),
            result => return result,
        }
    }
}

fn receive_u64(channel: Handle) -> Option<u64> {
    let mut reply = [0u8; 16];
    let (length, _) = taaj::receive(channel, &mut reply, &mut []).ok()?;
    (length == 8).then(|| u64::from_le_bytes(reply[..8].try_into().unwrap_or([0; 8])))
}

/// The median and the 99th percentile of `times`, in ns.
fn spread(times: &mut [u64]) -> (u64, u64) {
    times.sort_unstable();
    (times[times.len() / 2], times[times.len() * 99 / 100])
}

/// Round trips of 16 bytes to the other end; their times in ns.
fn round_trips(to: Handle, from: Handle) -> Option<Vec<u64>> {
    let mut request = [0u8; 16];
    request[0] = ROUND_TRIP;
    let mut reply = [0u8; 16];
    let mut times = Vec::with_capacity(ROUND_TRIPS);
    for _ in 0..ROUND_TRIPS {
        let start = taaj::monotonic_ns();
        send(to, &request, &[]).ok()?;
        taaj::receive(from, &mut reply, &mut []).ok()?;
        times.push(taaj::monotonic_ns() - start);
    }
    Some(times)
}

/// MB/s (bytes per microsecond) for `bytes` in `ns`.
fn megabytes_per_second(bytes: u64, ns: u64) -> u64 {
    bytes * 1000 / ns.max(1)
}

fn all() -> i64 {
    let started = taaj::monotonic_ns();
    let cpus = taaj::services()
        .and_then(|s| s.lookup("system").ok())
        .and_then(|system| taaj::system_info(system).ok())
        .map_or(0, |info| info.cpus);
    if cpus > 0 {
        println!("ipcbench: on {cpus} CPUs");
    }

    // The other program: requests to it on one channel, replies from it
    // on another.
    let (Ok((to_it, its_requests)), Ok((its_replies, from_it))) = (taaj::channel_create(), taaj::channel_create()) else {
        eprintln!("ipcbench: no channels");
        return 1;
    };
    let given = taaj::start::Given { input: Some(its_requests), output: Some(its_replies) };
    let process = match taaj::start::start("ipcbench", &["serve"], given) {
        Ok(process) => process,
        Err(why) => {
            eprintln!("ipcbench: could not start the other end: {why}");
            return 1;
        }
    };

    // 1. Round trips, to the other program.
    let Some(mut times) = round_trips(to_it, from_it) else {
        eprintln!("ipcbench: the round trips to the other program failed");
        return 1;
    };
    let (median, p99) = spread(&mut times);
    println!("ipcbench: round trip, two programs, 16 bytes: median {median} ns, 99% {p99} ns ({ROUND_TRIPS} times)");

    // 2. Round trips, to a thread of this program.
    let (Ok((to_thread, thread_requests)), Ok((thread_replies, from_thread))) = (taaj::channel_create(), taaj::channel_create()) else {
        eprintln!("ipcbench: no channels");
        return 1;
    };
    let Ok(thread) = taaj::thread::spawn(move || serve(thread_requests, thread_replies)) else {
        eprintln!("ipcbench: could not start a thread");
        return 1;
    };
    match round_trips(to_thread, from_thread) {
        Some(mut times) => {
            let (median, p99) = spread(&mut times);
            println!("ipcbench: round trip, two threads, 16 bytes: median {median} ns, 99% {p99} ns ({ROUND_TRIPS} times)");
        }
        None => println!("ipcbench: the round trips to the thread failed"),
    }
    let _ = send(to_thread, &[QUIT], &[]);
    thread.join();

    // 3. Bulk data through messages, to the other program.
    let mut message = [0u8; MESSAGE];
    message[0] = BULK;
    let start = taaj::monotonic_ns();
    for _ in 0..BULK_BYTES / MESSAGE {
        if send(to_it, &message, &[]).is_err() {
            eprintln!("ipcbench: a bulk message could not be sent");
            return 1;
        }
    }
    let _ = send(to_it, &[BULK_END], &[]);
    let got = receive_u64(from_it).unwrap_or(0);
    let ns = taaj::monotonic_ns() - start;
    if got != BULK_BYTES as u64 {
        println!("ipcbench: messages: FAILED - {got} bytes arrived of {BULK_BYTES}");
    } else {
        println!("ipcbench: messages, {MESSAGE} bytes: {} MB/s ({} MiB)", megabytes_per_second(got, ns), BULK_BYTES >> 20);
    }

    // 4. Bulk data through a shared memory object: written here, read
    // and summed there - the sum says it all arrived.
    let shared = match taaj::memory_create(SHARED).and_then(|m| Ok((m, taaj::memory_map(m, true)?))) {
        Ok(shared) => shared,
        Err(error) => {
            eprintln!("ipcbench: no shared memory: {error:?}");
            return 1;
        }
    };
    let (memory, base) = shared;
    let lent = taaj::duplicate(memory, abi::rights::READ | abi::rights::TRANSFER);
    let mut ok = lent.is_ok_and(|lent| send(to_it, &[MEMORY], &[lent]).is_ok());
    let mut reply = [0u8; 16];
    ok = ok && taaj::receive(from_it, &mut reply, &mut []).is_ok_and(|(n, _)| &reply[..n] == b"ok");
    let start = taaj::monotonic_ns();
    for round in 0..SHARED_ROUNDS {
        // SAFETY: SHARED bytes, mapped writable above.
        let words = unsafe { core::slice::from_raw_parts_mut(base.cast::<u64>(), (SHARED / 8) as usize) };
        let mut sum = 0u64;
        for (n, word) in words.iter_mut().enumerate() {
            *word = round << 32 | n as u64;
            sum = sum.wrapping_add(*word);
        }
        ok = ok && send(to_it, &[GO], &[]).is_ok() && receive_u64(from_it) == Some(sum);
    }
    let ns = taaj::monotonic_ns() - start;
    if ok {
        let bytes = SHARED * SHARED_ROUNDS;
        println!("ipcbench: shared memory, 1 MiB at a time: {} MB/s ({} MiB), every byte checked", megabytes_per_second(bytes, ns), bytes >> 20);
    } else {
        println!("ipcbench: shared memory: FAILED - the other end did not read what was written");
    }

    let _ = send(to_it, &[QUIT], &[]);
    let _ = taaj::wait_any(&[process]);
    println!("ipcbench: done in {} ms", (taaj::monotonic_ns() - started) / 1_000_000);
    0
}

fn main() -> i64 {
    let arguments = taaj::arguments();
    match arguments.first().map(alloc::string::String::as_str) {
        Some("all") => all(),
        Some("serve") => match (taaj::stdin(), taaj::stdout()) {
            (Some(requests), Some(replies)) => {
                serve(requests, replies);
                0
            }
            _ => 2,
        },
        _ => {
            eprintln!("usage: ipcbench all");
            2
        }
    }
}
