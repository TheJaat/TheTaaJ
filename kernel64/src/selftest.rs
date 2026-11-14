//! Checks the kernel runs on itself at every boot.
//!
//! The host tests prove the algorithms; these prove the wiring - that
//! the real frame allocator, the real page tables and the real heap work
//! on the real machine. Each one asserts, so a failure is a kernel panic
//! with the exact line, and `make boot64-test` fails on it.

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use kernel64::memory::addr::{phys_to_virt, VirtAddr};
use kernel64::paging::{Flags, PageSize};

use crate::arch::{self, mmu};
use crate::irqlock::IrqLock;
use crate::{interrupts, mm, threads};

pub fn run() {
    frames();
    paging();
    protections();
    heap();
    kprintln!("[selftest] memory: all passed");
}

fn frames() {
    mm::with_memory(|_, frames| {
        let before = frames.free_frames();
        let a = frames.allocate().expect("no free frame");
        let b = frames.allocate().expect("no free frame");
        assert_ne!(a, b);
        frames.free(a);
        assert_eq!(frames.allocate(), Some(a), "a freed frame is reused first");
        frames.free(a);
        frames.free(b);
        assert_eq!(frames.free_frames(), before);
    });
    kprintln!("[selftest] frames: allocate, free, reuse");
}

/// An address nothing else uses.
const TEST_PAGE: u64 = 0xFFFF_D000_0000_0000;

fn paging() {
    mm::with_memory(|tables, frames| {
        let virt = VirtAddr::new(TEST_PAGE);
        let frame = frames.allocate().expect("no free frame");
        tables
            .map(virt, frame, PageSize::Size4K, Flags::WRITABLE | mm::no_execute(), frames)
            .expect("mapping the test page failed");

        // Write through the new mapping, read through the direct map:
        // the same physical memory, seen at two virtual addresses.
        let page = virt.as_mut_ptr::<u64>();
        let alias = phys_to_virt(frame).as_ptr::<u64>();
        // SAFETY: both point at the frame just mapped, which nothing
        // else uses.
        unsafe {
            page.write_volatile(0x5441_414A_0000_0001);
            assert_eq!(alias.read_volatile(), 0x5441_414A_0000_0001);
        }
        assert_eq!(tables.translate(virt).map(|t| t.physical), Some(frame));

        assert_eq!(tables.unmap(virt), Some(frame));
        mmu::flush(TEST_PAGE);
        assert_eq!(tables.translate(virt), None);
        frames.free(frame);
    });
    kprintln!("[selftest] paging: map, alias through the direct map, unmap");
}

/// An immutable static, so it lands in .rodata.
static READ_ONLY: u64 = 7;
/// A mutable one, in .data/.bss.
static WRITABLE: AtomicU64 = AtomicU64::new(0);

/// The kernel's own mappings have the permissions linker.ld intends.
fn protections() {
    mm::with_memory(|tables, _| {
        let flags = |address: u64| tables.translate(VirtAddr::new(address)).map(|t| t.flags);
        let nx = mm::no_execute();

        let code = flags(run as *const () as u64).expect("kernel code is unmapped");
        assert!(!code.contains(Flags::WRITABLE) && !code.contains(Flags::NO_EXECUTE), "code is {code:?}");

        let rodata = flags(&raw const READ_ONLY as u64).expect("rodata is unmapped");
        assert!(!rodata.contains(Flags::WRITABLE) && rodata.contains(nx), "rodata is {rodata:?}");

        WRITABLE.fetch_add(1, Ordering::Relaxed);
        let data = flags(&raw const WRITABLE as u64).expect("data is unmapped");
        assert!(data.contains(Flags::WRITABLE | nx), "data is {data:?}");

        assert_eq!(flags(0), None, "the null page must be unmapped");
        assert_eq!(flags(mm::kernel_layout().stack_guard), None, "the stack guard must be unmapped");
    });
    kprintln!("[selftest] protections: code RX, rodata R, data RW+NX, null and stack guard unmapped");
}

fn heap() {
    let (used_before, total_before) = mm::heap_usage();
    {
        let boxed = Box::new(42u64);
        assert_eq!(*boxed, 42);

        // 4 MiB - well past the initial 1 MiB, so the heap has to grow.
        let numbers: Vec<u64> = (0..512 * 1024).collect();
        let sum: u64 = numbers.iter().sum();
        assert_eq!(sum, 512 * 1024 * (512 * 1024 - 1) / 2);

        let mut text = String::from("Hello");
        text.push_str(", heap");
        assert_eq!(format!("{text}!"), "Hello, heap!");

        let (_, total) = mm::heap_usage();
        assert!(total > total_before, "the heap did not grow");
        kprintln!("[selftest] heap: Box, Vec, String; grew from {} KiB to {} KiB", total_before / 1024, total / 1024);
    }
    let (used_after, _) = mm::heap_usage();
    assert_eq!(used_after, used_before, "dropping everything did not free everything");
}

/// The timer interrupt arrives, at the rate it was set to: ten ticks,
/// timed with the TSC (calibrated separately, against the PIT).
pub fn timer() {
    let tsc_hz = arch::platform::tsc_hz();
    let first = interrupts::ticks();
    // Line up with a tick boundary, then count ten.
    while interrupts::ticks() == first {
        arch::wait_for_interrupt();
    }
    let start_ticks = interrupts::ticks();
    let start = arch::tsc();
    while interrupts::ticks() < start_ticks + 10 {
        arch::wait_for_interrupt();
    }
    let ms = (arch::tsc() - start) * 1000 / tsc_hz.max(1);
    let expected = 10 * 1000 / interrupts::HZ;
    // Generous bounds: a VM's clocks can stretch under load.
    assert!(ms >= expected / 2 && ms <= expected * 3, "10 timer ticks took {ms} ms, expected {expected}");
    kprintln!("[selftest] timer: 10 ticks in {ms} ms (expected {expected})");
}

/// Threads: they take turns, a thread that never yields is still
/// interrupted, sleep sleeps, and dead threads give everything back.
pub fn threads() {
    // 1. Taking turns: two threads that yield after every step.
    let log = Arc::new(IrqLock::new(Vec::new()));
    let ids: Vec<_> = (*b"ab")
        .into_iter()
        .map(|tag| {
            let log = Arc::clone(&log);
            threads::spawn("turns", move || {
                for _ in 0..3 {
                    log.lock().push(tag);
                    threads::yield_now();
                }
            })
        })
        .collect();
    for id in ids {
        threads::join(id);
    }
    let log = log.lock().clone();
    assert_eq!(log.iter().filter(|&&b| b == b'a').count(), 3);
    assert_eq!(log.iter().filter(|&&b| b == b'b').count(), 3);
    assert_ne!(log, b"aaabbb", "yield did not hand over the CPU");
    kprintln!("[selftest] threads: two yielding threads ran as {:?}", core::str::from_utf8(&log).unwrap());

    // 2. Preemption: this thread never yields. If the timer did not take
    // the CPU from it, nothing else - including this test - would ever
    // run again; it gives up after two seconds and says so.
    let stop = Arc::new(AtomicBool::new(false));
    let spinner = {
        let stop = Arc::clone(&stop);
        threads::spawn("spinner", move || {
            let deadline = arch::tsc() + 2 * arch::platform::tsc_hz();
            while !stop.load(Ordering::Relaxed) {
                assert!(arch::tsc() < deadline, "the spinning thread was never preempted");
                core::hint::spin_loop();
            }
        })
    };
    threads::sleep(3); // only returns if the spinner is preempted
    stop.store(true, Ordering::Relaxed);
    threads::join(spinner);
    kprintln!("[selftest] threads: a thread that never yields was preempted");

    // 3. Sleeping.
    let start = interrupts::ticks();
    threads::sleep(5);
    let slept = interrupts::ticks() - start;
    assert!((5..=7).contains(&slept), "sleep(5) took {slept} ticks");

    // 4. Churn: 100 threads come and go. Their stacks must be recycled -
    // no growth in slots or frames after the first round.
    //
    // All 100 wait at a gate until every one exists. Otherwise the peak
    // number alive would depend on timing - the timer can preempt the
    // spawning loop and let early threads finish - and a second round
    // that happened to peak higher would reach a stack slot never used
    // before, whose page table stays: one frame that looks like a leak.
    let churn = || {
        let gate = Arc::new(AtomicBool::new(false));
        let ids: Vec<_> = (0..100)
            .map(|n| {
                let gate = Arc::clone(&gate);
                threads::spawn("churn", move || {
                    while !gate.load(Ordering::Acquire) {
                        threads::yield_now();
                    }
                    let v: Vec<u64> = (0..n).collect();
                    assert_eq!(v.len() as u64, n);
                })
            })
            .collect();
        gate.store(true, Ordering::Release);
        for id in ids {
            threads::join(id);
        }
        threads::yield_now(); // let the last exited thread be reaped
    };
    churn();
    let frames = mm::with_memory(|_, f| f.free_frames());
    let slots = mm::stack_slots_used();
    churn();
    assert_eq!(mm::with_memory(|_, f| f.free_frames()), frames, "thread churn leaked frames");
    assert_eq!(mm::stack_slots_used(), slots, "thread churn leaked stack slots");
    assert_eq!(threads::count(), 2, "only boot and idle should remain");
    kprintln!("[selftest] threads: sleep(5) took {slept} ticks; 200 threads spawned and reaped, nothing leaked");
}

/// User mode: four programs, run at once. Two follow the rules and must
/// finish with their exit codes; two break them and must be killed -
/// without the kernel noticing anything but a log line. Then everything
/// they used must be back.
pub fn user() {
    use crate::{process, programs};

    let run_all = || {
        let programs: [(&'static str, &[u8]); 7] = [
            ("hello", programs::HELLO),
            ("peek", programs::PEEK),
            ("cli", programs::CLI),
            ("port", programs::PORT),
            ("memro", programs::MEMRO),
            ("spin", programs::SPIN),
            ("heapcheck", programs::HEAPCHECK),
        ];
        let started: Vec<_> = programs
            .iter()
            .map(|&(name, file)| process::spawn(name, file, Vec::new()).unwrap_or_else(|e| panic!("loading {name}: {e:?}")))
            .collect();
        started
            .into_iter()
            .map(|(process, thread)| {
                threads::join(thread);
                (process.name, process.exit_code())
            })
            .collect::<Vec<_>>()
    };

    let results = run_all();
    let expected = [
        ("hello", Some(42)),
        ("peek", Some(abi::KILLED)),
        ("cli", Some(abi::KILLED)),
        ("port", Some(abi::KILLED)),
        ("memro", Some(abi::KILLED)),
        ("spin", Some(0)),
        ("heapcheck", Some(0)),
    ];
    assert_eq!(results, expected, "user programs did not end as expected");

    // A second round must leave memory exactly as the first did: every
    // page of every process - code, data, stack, page tables - returned.
    threads::yield_now(); // let the last exited thread be reaped
    let frames = mm::with_memory(|_, f| f.free_frames());
    assert_eq!(run_all(), expected);
    threads::yield_now();
    assert_eq!(mm::with_memory(|_, f| f.free_frames()), frames, "processes leaked frames");
    kprintln!("[selftest] user: hello exited 42, spin and heapcheck exited 0, peek, cli, port and memro were killed; 14 processes, nothing leaked");
}

/// IPC: a service and its client, connected by one channel the kernel
/// made. The client tests every rule of handles and channels, then asks
/// the service to crash and must survive it. Twice - and then every
/// channel and every frame must be gone.
pub fn ipc() {
    use crate::{ipc, process, programs};
    use kernel64::cap::Rights;

    let run = || {
        let (service_end, client_end) = ipc::channel();
        let (service, service_thread) =
            process::spawn("echod", programs::ECHOD, alloc::vec![(service_end, Rights::ALL)]).expect("loading echod");
        let (client, client_thread) =
            process::spawn("echoc", programs::ECHOC, alloc::vec![(client_end, Rights::ALL)]).expect("loading echoc");
        threads::join(client_thread);
        threads::join(service_thread);
        assert_eq!(client.exit_code(), Some(0), "echoc failed (the code is the line of the failed check)");
        assert_eq!(service.exit_code(), Some(abi::KILLED), "echod should have crashed");
    };

    run();
    threads::yield_now();
    let frames = mm::with_memory(|_, f| f.free_frames());
    run();
    threads::yield_now();
    assert_eq!(mm::with_memory(|_, f| f.free_frames()), frames, "IPC leaked frames");
    assert_eq!(ipc::live_channels(), 0, "channels leaked");
    kprintln!("[selftest] ipc: echo service and client - rights, transfer, backpressure, crash isolation; 0 channels left");
}

/// Files: the file service and a client that tests it - long files,
/// listings, refusals, careless requests. When the client is done it
/// closes its end, and the service must end cleanly. Twice, and then
/// nothing may be left: no channel (each request made one for its
/// reply), no frame (the service's heap held every file).
pub fn files() {
    use crate::{ipc, process, programs};
    use kernel64::cap::Rights;

    let run = || {
        let (service_end, client_end) = ipc::channel();
        let (service, service_thread) =
            process::spawn("fs", programs::FS, alloc::vec![(service_end, Rights::ALL)]).expect("loading fs");
        let (client, client_thread) =
            process::spawn("fscheck", programs::FSCHECK, alloc::vec![(client_end, Rights::ALL)]).expect("loading fscheck");
        threads::join(client_thread);
        threads::join(service_thread);
        assert_eq!(client.exit_code(), Some(0), "fscheck failed (the code is the line of the failed check)");
        assert_eq!(service.exit_code(), Some(0), "fs should end when its last client goes");
    };

    run();
    threads::yield_now();
    let frames = mm::with_memory(|_, f| f.free_frames());
    run();
    threads::yield_now();
    assert_eq!(mm::with_memory(|_, f| f.free_frames()), frames, "the file service leaked frames");
    assert_eq!(ipc::live_channels(), 0, "channels leaked");
    kprintln!("[selftest] files: fs and fscheck, twice - every check passed, 0 channels and no frames left");
}
