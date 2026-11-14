//! Kernel threads: the mechanism around `kernel64::sched`'s decisions.
//!
//! Each thread has its own kernel stack (with a guard page below it), a
//! saved stack pointer while it is not running, and - until it starts -
//! the closure it will run. The scheduler policy says who runs next;
//! this module makes it happen with `arch::context::switch`.
//!
//! Everything that touches the thread table or switches runs with
//! interrupts disabled: a timer interrupt arriving halfway through a
//! switch would find the bookkeeping half-updated. One CPU for now; SMP
//! will need per-CPU current threads and run queues.

use alloc::boxed::Box;
use alloc::collections::{BTreeMap, VecDeque};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

use kernel64::sched::{Scheduler, State, Switch, ThreadId};

use crate::arch::{self, context, mmu, syscall};
use crate::process::Process;
use crate::irqlock::IrqLock;
use crate::mm::{self, KernelStack};
use crate::interrupts;

type Entry = Box<dyn FnOnce() + Send>;

struct Thread {
    name: &'static str,
    /// Saved while the thread is not running. Written by the switch.
    rsp: u64,
    /// Owned for its Drop, which unmaps and frees the stack once the
    /// thread is reaped. None for the boot thread (the boot stack).
    _stack: Option<KernelStack>,
    /// The top of that stack: where the CPU lands on a system call or an
    /// interrupt from user mode.
    stack_top: Option<u64>,
    /// The process whose address space this thread runs in; None for
    /// kernel threads, which use the kernel's.
    process: Option<Arc<Process>>,
    entry: Option<Entry>,
}

/// The address space loaded in CR3 now, to skip reloading the same one
/// (a reload flushes the TLB).
static ACTIVE_ROOT: AtomicU64 = AtomicU64::new(0);

struct Threads {
    policy: Scheduler,
    /// Boxed so each Thread stays put: the switch writes `rsp` through a
    /// raw pointer after the table's lock is released.
    table: BTreeMap<ThreadId, Box<Thread>>,
    next_id: ThreadId,
}

static THREADS: IrqLock<Option<Threads>> = IrqLock::new(None);

/// Wakes everyone waiting in `join` whenever any thread exits.
static EXITED: WaitQueue = WaitQueue::new();

const IDLE: ThreadId = 0;
const BOOT: ThreadId = 1;

/// Turns the code running now into thread 1, "boot", and creates thread
/// 0, the idle thread, which runs whenever nothing else is ready.
pub fn init() {
    let stack = mm::allocate_stack().expect("no stack for the idle thread");
    // SAFETY: a freshly mapped stack, nobody else's.
    let rsp = unsafe { context::initial_stack(stack.top()) };
    let stack_top = Some(stack.top());
    let idle = Thread {
        name: "idle",
        rsp,
        _stack: Some(stack),
        stack_top,
        process: None,
        entry: Some(Box::new(idle_loop)),
    };
    let boot = Thread { name: "boot", rsp: 0, _stack: None, stack_top: None, process: None, entry: None };
    ACTIVE_ROOT.store(mm::kernel_root(), Ordering::Relaxed);

    let mut table = BTreeMap::new();
    table.insert(IDLE, Box::new(idle));
    table.insert(BOOT, Box::new(boot));
    *THREADS.lock() = Some(Threads { policy: Scheduler::with_running(IDLE, BOOT), table, next_id: 2 });
}

/// The idle thread: sleeps until an interrupt. If the interrupt makes a
/// thread ready, the switch happens on the way out of the interrupt.
fn idle_loop() {
    loop {
        arch::wait_for_interrupt();
    }
}

fn with<R>(f: impl FnOnce(&mut Threads) -> R) -> R {
    f(THREADS.lock().as_mut().expect("threads not initialised"))
}

/// Starts `f` on a new kernel thread.
pub fn spawn(name: &'static str, f: impl FnOnce() + Send + 'static) -> ThreadId {
    spawn_in(None, name, f)
}

/// Starts `f` on a new thread in `process`'s address space (or the
/// kernel's, for None).
pub fn spawn_in(
    process: Option<Arc<Process>>,
    name: &'static str,
    f: impl FnOnce() + Send + 'static,
) -> ThreadId {
    let stack = mm::allocate_stack().expect("out of kernel stacks");
    // SAFETY: a freshly mapped stack, nobody else's.
    let rsp = unsafe { context::initial_stack(stack.top()) };
    let stack_top = Some(stack.top());
    let thread =
        Box::new(Thread { name, rsp, _stack: Some(stack), stack_top, process, entry: Some(Box::new(f)) });
    with(|t| {
        let id = t.next_id;
        t.next_id += 1;
        t.table.insert(id, thread);
        t.policy.add(id);
        id
    })
}

pub fn current() -> ThreadId {
    with(|t| t.policy.current())
}

/// The process the running thread belongs to, if any.
pub fn current_process() -> Option<Arc<Process>> {
    with(|t| t.table.get(&t.policy.current()).and_then(|thread| thread.process.clone()))
}

pub fn name(id: ThreadId) -> Option<&'static str> {
    with(|t| t.table.get(&id).map(|thread| thread.name))
}

pub fn count() -> usize {
    with(|t| t.policy.thread_count())
}

/// Lets the scheduler pick again - possibly this thread again, if nobody
/// else is ready.
pub fn yield_now() {
    schedule();
}

/// Takes the current thread off the CPU in `state`, and runs whoever is
/// next. Returns when this thread runs again.
fn suspend(state: State) {
    let were_enabled = arch::disable_interrupts();
    with(|t| t.policy.suspend_current(state));
    schedule();
    arch::restore_interrupts(were_enabled);
}

/// Sleeps for at least `ticks` timer ticks.
pub fn sleep(ticks: u64) {
    suspend(State::Sleeping(interrupts::ticks() + ticks));
}

/// Ends the current thread.
pub fn exit() -> ! {
    arch::disable_interrupts();
    with(|t| t.policy.suspend_current(State::Exited));
    EXITED.wake_all();
    schedule();
    unreachable!("an exited thread was scheduled again");
}

/// Waits until thread `id` has exited.
pub fn join(id: ThreadId) {
    EXITED.wait_while(|| with(|t| matches!(t.policy.state(id), Some(s) if s != State::Exited)));
}

/// Makes a blocked or sleeping thread ready.
pub fn wake(id: ThreadId) -> bool {
    with(|t| t.policy.wake(id))
}

/// Asks the policy for the next thread and switches to it, if it is a
/// different one.
fn schedule() {
    let were_enabled = arch::disable_interrupts();
    let switch = with(|t| {
        let Switch { from, to } = t.policy.pick_next()?;
        let save = &raw mut t.table.get_mut(&from).expect("current thread missing").rsp;
        let next = t.table.get(&to).expect("next thread missing");
        let root = next.process.as_ref().map_or_else(mm::kernel_root, |p| p.root());
        Some((save, next.rsp, root, next.stack_top))
    });
    if let Some((save, load, root, stack_top)) = switch {
        // The next thread's world: its kernel stack for entries from user
        // mode, and its address space. Both are safe to change here - this
        // code and its stack are in the kernel half, mapped in every
        // address space.
        if let Some(top) = stack_top {
            syscall::set_kernel_stack(top);
        }
        if ACTIVE_ROOT.swap(root, Ordering::Relaxed) != root {
            // SAFETY: a process's tables share the kernel half.
            unsafe { mmu::activate(root) };
        }
        // SAFETY: interrupts are off; both threads are in the table, and
        // neither record is freed while it is involved in a switch (reap
        // never frees the current thread).
        unsafe { context::switch(save, load) };
    }
    // Running again - possibly much later.
    reap();
    arch::restore_interrupts(were_enabled);
}

/// Frees the stacks of exited threads. A thread cannot free the stack it
/// is running on, so the next thread to run does it.
fn reap() {
    let dead: Vec<Box<Thread>> = with(|t| {
        let exited: Vec<ThreadId> =
            t.table.keys().copied().filter(|&id| t.policy.remove(id)).collect();
        exited.iter().filter_map(|id| t.table.remove(id)).collect()
    });
    drop(dead); // outside the lock: unmapping stacks takes other locks
}

/// Called by the timer interrupt.
pub fn on_tick(now: u64) {
    if let Some(t) = THREADS.lock().as_mut() {
        t.policy.tick(now);
    }
}

/// Called at the end of every interrupt: switches if a slice ran out or a
/// wakeup made a thread ready while the CPU idled. Runs on the
/// interrupted thread's stack; the interrupt returns when this thread is
/// scheduled again.
pub fn preempt_if_needed() {
    let due = THREADS.lock().as_mut().is_some_and(|t| t.policy.take_reschedule());
    if due {
        schedule();
    }
}

/// Where every new thread starts (from `thread_trampoline`).
pub extern "C" fn thread_entry() -> ! {
    // A thread may first be switched to from inside an interrupt, with
    // interrupts still off; a running thread has them on.
    reap();
    let entry = with(|t| {
        let id = t.policy.current();
        t.table.get_mut(&id).and_then(|thread| thread.entry.take())
    });
    arch::enable_interrupts();
    if let Some(entry) = entry {
        entry();
    }
    exit()
}

/// Threads waiting for something - a key, a message, another thread's
/// exit.
pub struct WaitQueue {
    waiters: IrqLock<VecDeque<ThreadId>>,
}

impl WaitQueue {
    pub const fn new() -> WaitQueue {
        WaitQueue { waiters: IrqLock::new(VecDeque::new()) }
    }

    /// Blocks the current thread for as long as `condition` holds.
    ///
    /// The check and the blocking happen with interrupts off, so a wakeup
    /// cannot slip in between them and be lost - the classic bug of
    /// "check, then sleep": the event arrives after the check, and the
    /// sleeper never hears of it.
    pub fn wait_while(&self, mut condition: impl FnMut() -> bool) {
        loop {
            let were_enabled = arch::disable_interrupts();
            if !condition() {
                arch::restore_interrupts(were_enabled);
                return;
            }
            let me = current();
            {
                let mut waiters = self.waiters.lock();
                if !waiters.contains(&me) {
                    waiters.push_back(me);
                }
            }
            suspend(State::Blocked);
            arch::restore_interrupts(were_enabled);
        }
    }

    /// Wakes every waiter; each re-checks its condition.
    pub fn wake_all(&self) {
        let waiters: VecDeque<ThreadId> = core::mem::take(&mut *self.waiters.lock());
        for id in waiters {
            wake(id);
        }
    }
}
