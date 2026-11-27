//! fuzz: makes system calls at random, to find what the kernel does not
//! check (K104).
//!
//!   with programs fuzz [CHILDREN [CALLS [SEED|random [trace]]]]
//!
//! The kernel must survive anything a program asks of it: whatever the
//! call, whatever the arguments, the worst a program may do is end
//! itself. This starts CHILDREN copies of itself (100), each with **no
//! handles at all** - no screen, no files, no services: the least a
//! program can have - and each makes CALLS system calls (3000) with
//! numbers and arguments from a generator seeded with SEED plus its
//! number (1, or one from the kernel with `random`). The same seed
//! makes the same calls, so a crash found is a crash repeated.
//!
//! The arguments lean towards what finds bugs: 0, -1, the largest sizes,
//! addresses in the kernel's half, unmapped ones, ones just past the end
//! of a page of the program's own, small handle numbers. Some calls end
//! a child (a garbage `signal_return`, a thread started at a random
//! address) and some wait for ever; each child gets two seconds, and is
//! stopped after that. Then this says how they ended.
//!
//! A kernel bug shows as a panic (the machine stops) or a hang (this
//! never finishes); the boot test checks for both (fuzz-test.sh).

#![no_std]
#![no_main]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

use taaj::{println, rights, Handle};

taaj::program!(main);

/// Ticks a child may run: two seconds, at 100 ticks a second.
const CHILD_TICKS: u64 = 200;
/// What a child may use of memory: random reservations must not take
/// the machine's, which other programs need (and the test is of the
/// kernel, not of running out).
const CHILD_MEMORY: u64 = 32 << 20;
/// The calls a child never makes: they would only end it early, which
/// costs a start and finds nothing - exit, and the last thread's exit.
const NEVER: [u64; 2] = [abi::call::EXIT, abi::call::THREAD_EXIT];
/// Calls that end the child that makes them, sooner or later: a signal
/// sent to itself, a handler or a signal stack at a random address, a
/// thread started at one, a return from a signal to a made-up context,
/// Each is drawn one time in 4096 - all of them together, about one child
/// in nine of 3000 calls (at one time in 64, `signal_return` alone killed
/// 32 children of 40) - so that most children make all their calls. They
/// are made, not left out: their checks are fuzzed too.
const RARE: [u64; 12] = [
    abi::call::SIGNAL_SEND,
    abi::call::SIGNAL_STACK,
    abi::call::PROCESS_NOTIFY,
    abi::call::MEMORY_LIMIT,
    abi::call::THREAD_CREATE,
    abi::call::THREAD_POINTER,
    abi::call::SIGNAL_SETUP,
    abi::call::SIGNAL_RETURN,
    abi::call::ALARM,
    abi::call::MEMORY_UNMAP,
    abi::call::MEMORY_PROTECT,
    abi::call::MEMORY_DISCARD,
];
/// Calls that may wait - for a message, a process, an interrupt, a futex,
/// or ticks - and with random arguments often wait for ever: drawn one
/// time in 4096: in a child's 3000 calls, about one child in eight
/// meets one. A child
/// that waits is stopped after its two seconds, which is a fine end too.
const WAITING: [u64; 8] = [
    abi::call::SLEEP,
    // A send waits while the channel's queue is full - and the channels
    // a child makes itself fill, with nobody reading.
    abi::call::CHANNEL_SEND,
    abi::call::CHANNEL_RECEIVE,
    abi::call::INTERRUPT_WAIT,
    abi::call::PROCESS_WAIT,
    abi::call::OBJECT_WAIT,
    abi::call::OBJECT_WAIT_UNTIL,
    abi::call::FUTEX_WAIT,
];
/// Call numbers are drawn below this: every call there is, and some that
/// are not (the kernel must refuse an unknown number too).
const NUMBERS: u64 = 80;

fn main() -> i64 {
    let arguments = taaj::arguments();
    if arguments.first().map(String::as_str) == Some("child") {
        return child(&arguments[1..]);
    }
    parent(&arguments)
}

// ---- the children -------------------------------------------------------

/// The memory calls may be pointed at: a reservation of the child's own,
/// which the kernel may read and write, as asked - into it, or across its
/// end. A reservation, not a static: past its end is unmapped (the kernel
/// leaves a page below every reservation), so a call that runs on is
/// refused there. A static had the program's own state after it, and a
/// `random(end - 2, 40 KiB)` wrote over it - the kernel did as it was
/// asked, and the child hung.
const SCRATCH: u64 = 64 * 1024;

/// xorshift64*: small, fast, and the same numbers from the same seed.
struct Random(u64);

impl Random {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    /// One argument, of a kind chosen at random.
    fn argument(&mut self, scratch: u64, may_point_here: bool) -> u64 {
        match self.below(12) {
            0 => 0,
            1 => 1,
            2 => self.below(64),         // a small number: a handle, a count
            3 => u64::MAX,               // -1
            4 => 1 << (self.below(63)),  // a power of two: a size
            5 => self.next(),            // anything
            6 => 0xFFFF_FFFF_8010_0000 + self.below(1 << 20), // the kernel's code
            7 => 0xFFFF_8000_0000_0000 + self.below(1 << 30), // the kernel's half
            8 => 0x1000 * self.below(16),                     // low, unmapped
            9 if may_point_here => scratch + self.below(SCRATCH),
            10 if may_point_here => scratch + SCRATCH - self.below(16), // across its end
            _ => self.below(1 << 16),
        }
    }
}

/// `fuzz child SEED CALLS`: makes the calls, then ends with 0.
fn child(arguments: &[String]) -> i64 {
    let seed = arguments.first().and_then(|s| s.parse().ok()).unwrap_or(1u64);
    let calls = arguments.get(1).and_then(|s| s.parse().ok()).unwrap_or(3_000u64);
    // `trace`: each call written to the kernel's console before it is
    // made - to find the one a child stopped in (it has no other output).
    let trace = arguments.get(2).map(String::as_str) == Some("trace");
    taaj::memory_limit(CHILD_MEMORY);
    let Ok(scratch) = taaj::memory_reserve(SCRATCH, true).map(|p| p as u64) else { return 3 };
    // Never 0: xorshift stays at 0 for ever.
    let mut random = Random(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let mut made = 0;
    while made < calls {
        let number = random.below(NUMBERS);
        if NEVER.contains(&number)
            || (RARE.contains(&number) && random.below(4096) != 0)
            || (WAITING.contains(&number) && random.below(4096) != 0)
        {
            continue;
        }
        // debug_write is the kernel's console: random bytes from the
        // scratch area would fill the log the tests read. Its checks of
        // the pointer are what matters, and bad pointers test those.
        let here = number != abi::call::DEBUG_WRITE;
        let a: [u64; 6] = core::array::from_fn(|_| random.argument(scratch, here));
        if trace {
            let line = alloc::format!("fuzz: call {made}: {number}({:#x}, {:#x}, {:#x}, {:#x}, {:#x}, {:#x})\n", a[0], a[1], a[2], a[3], a[4], a[5]);
            taaj::syscall(abi::call::DEBUG_WRITE, line.as_ptr() as u64, line.len() as u64, 0);
        }
        taaj::syscall6(number, a[0], a[1], a[2], a[3], a[4], a[5]);
        made += 1;
    }
    0
}

// ---- the parent -------------------------------------------------------

/// How the children ended.
#[derive(Default)]
struct Endings {
    finished: u64,
    killed: u64,
    stopped: u64,
    other: u64,
    not_started: u64,
    /// The codes of those that ended otherwise.
    codes: Vec<i64>,
}

/// `with programs fuzz CHILDREN CALLS SEED trace`: the children trace.
static TRACE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

fn parent(arguments: &[String]) -> i64 {
    TRACE.store(arguments.get(3).map(String::as_str) == Some("trace"), core::sync::atomic::Ordering::Relaxed);
    let children = arguments.first().and_then(|s| s.parse().ok()).unwrap_or(100u64);
    let calls = arguments.get(1).and_then(|s| s.parse().ok()).unwrap_or(3_000u64);
    let base = match arguments.get(2).map(String::as_str) {
        Some("random") => {
            let mut bytes = [0u8; 8];
            let _ = taaj::random(&mut bytes);
            u64::from_le_bytes(bytes) >> 16
        }
        Some(seed) => seed.parse().unwrap_or(1),
        None => 1,
    };
    let (Some(services), Some(files)) = (taaj::services(), taaj::file_service()) else {
        println!("fuzz: it starts programs: run it as `with programs fuzz`");
        return 2;
    };
    let Ok(init) = services.lookup("init") else {
        println!("fuzz: init would not be asked for");
        return 2;
    };
    // Its own file, read once, lent to init for each child.
    let file = taaj::command_line().file.unwrap_or_else(|| String::from("/fuzz"));
    let Some(image) = read(&files, file.trim_start_matches('/')) else {
        println!("fuzz: cannot read {file}");
        return 2;
    };

    println!("fuzz: {children} children, {calls} calls each, seeds {base} to {}", base + children - 1);
    let mut endings = Endings::default();
    let mut slow = Vec::new();
    for n in 0..children {
        let seed = base + n;
        let Some(process) = start(init, image, &file, seed, calls) else {
            endings.not_started += 1;
            continue;
        };
        let code = match taaj::object_wait_until(&[process], taaj::ticks() + CHILD_TICKS) {
            Ok(Some(_)) => taaj::wait_any(&[process]).map(|(_, code)| code).unwrap_or(i64::MIN),
            _ => {
                slow.push(seed);
                let _ = taaj::kill(process);
                taaj::wait_any(&[process]).map(|(_, code)| code).unwrap_or(i64::MIN)
            }
        };
        let _ = taaj::close(process);
        match code {
            0 => endings.finished += 1,
            abi::KILLED => endings.killed += 1,
            abi::STOPPED => endings.stopped += 1,
            code => {
                endings.other += 1;
                endings.codes.push(code);
            }
        }
    }
    let Endings { finished, killed, stopped, other, not_started, codes } = endings;
    println!(
        "fuzz: {finished} made all their calls, {killed} were killed for breaking a rule, {stopped} were stopped after {} s, {other} ended otherwise, {not_started} did not start",
        CHILD_TICKS / 100
    );
    if !codes.is_empty() {
        let codes: Vec<String> = codes.iter().map(|c| alloc::format!("{c}")).collect();
        println!("fuzz: the other codes: {}", codes.join(" "));
    }
    if !slow.is_empty() {
        let seeds: Vec<String> = slow.iter().map(|s| alloc::format!("{s}")).collect();
        println!("fuzz: still running when stopped: seeds {}", seeds.join(" "));
    }
    // A child that was killed or stopped said nothing of how far it got;
    // those that finished made all CALLS.
    println!("fuzz: done - {children} programs with no handles, {} calls from those that finished, the kernel still here", finished * calls);
    if not_started == 0 { 0 } else { 1 }
}

/// The file `path` in a memory object of its own.
fn read(files: &taaj::files::Files, path: &str) -> Option<Handle> {
    let size = files.size(path).ok()?;
    let memory = taaj::memory_create(size).ok()?;
    match files.read_into(path, memory) {
        Ok(got) if got == size => Some(memory),
        _ => {
            let _ = taaj::close(memory);
            None
        }
    }
}

/// Asks init to start `fuzz child SEED CALLS` from `image`, with no
/// handles: flags 0, and nothing but the image and the reply channel.
fn start(init: Handle, image: Handle, file: &str, seed: u64, calls: u64) -> Option<Handle> {
    let lent = taaj::duplicate(image, rights::READ | rights::TRANSFER).ok()?;
    let (reply, theirs) = taaj::channel_create().ok()?;
    let mut words = alloc::vec![String::from("child"), alloc::format!("{seed}"), alloc::format!("{calls}")];
    if TRACE.load(core::sync::atomic::Ordering::Relaxed) {
        words.push(String::from("trace"));
    }
    let mut encoded = [0u8; 64];
    let length = abi::arguments::encode(words.iter().map(String::as_str), &mut encoded)?;
    let request = abi::init::Run {
        kind: abi::init::IMAGE | abi::init::REPLY,
        flags: 0,
        name: "fuzz",
        directory: "",
        file,
        arguments: &encoded[..length],
    };
    let mut message = [0u8; abi::MESSAGE_MAX as usize];
    let size = request.encode(&mut message)?;
    if taaj::send(init, &message[..size], &[lent, theirs]).is_err() {
        let _ = (taaj::close(lent), taaj::close(theirs), taaj::close(reply));
        return None;
    }
    let mut answer = [0u8; 64];
    let mut handle = [0u64; 1];
    let process = match taaj::receive(reply, &mut answer, &mut handle) {
        Ok((2, 1)) if &answer[..2] == b"ok" => Some(handle[0]),
        _ => None,
    };
    let _ = taaj::close(reply);
    process
}
