//! POSIX threads: pthread_create and the rest of pthread.h, errno for
//! each thread, and C++'s emulated thread-local variables.
//!
//! The kernel gives a program threads (THREAD_CREATE), a thread pointer
//! for each (THREAD_POINTER) and futexes (FUTEX_WAIT, FUTEX_WAKE).
//! Everything else is here, in the program's own memory.
//!
//! **A thread** is a `Thread` record. Its thread pointer points at the
//! record itself, so the running thread finds its own in one
//! instruction: on x86_64 the record's first word holds its own address
//! and `mov rax, fs:0` reads it (the x86_64 ELF ABI asks for exactly
//! that word at FS:0); on AArch64 TPIDR_EL0 holds it (its first two
//! words are the ones the AArch64 ABI keeps for the system). A
//! `pthread_t` is the record's address.
//!
//! A thread's stack is memory reserved for it (MEMORY_RESERVE): a page
//! takes memory only once touched, and the unmapped page the kernel
//! leaves below every reservation makes an overflow fault instead of
//! running into whatever lies under it. The thread ends
//! with THREAD_EXIT naming its record's `running` word, which the
//! kernel sets to 0 and wakes once the thread will run no more:
//! `pthread_join` sleeps on that word, then unmaps the stack. A detached
//! thread can't free its own stack while it is on it, so it is put on a
//! list instead, and the next `pthread_create` or `pthread_detach`
//! frees whatever on the list has ended.
//!
//! **The locks** are futex locks that fit the sizes pthread.h has had
//! since TheTaaJ had one thread (libc++ was compiled with them):
//!
//! ```text
//!  pthread_mutex_t   [kind | depth << 8][owner's id | WAITING]   8 bytes
//!  pthread_cond_t    [sequence]                                  4 bytes
//!  pthread_rwlock_t  [readers, or WRITER][sequence]              8 bytes
//!  pthread_once_t    [0 not run, 1 running, 2 done]              4 bytes
//! ```
//!
//! A mutex nobody else wants is taken and given back with one atomic
//! instruction each; the kernel is asked only to sleep and to wake.

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::ffi::{c_int, c_long, c_void};
use core::ptr;
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use taaj::sync::Mutex;

use crate::errno::{EAGAIN, EBUSY, EDEADLK, EINVAL, EPERM, ESRCH, ETIMEDOUT};

/// pthread.h's PTHREAD_KEYS_MAX.
const KEYS_MAX: usize = 128;
/// How many times destructors are run at a thread's end while values
/// remain (PTHREAD_DESTRUCTOR_ITERATIONS).
const DESTRUCTOR_ROUNDS: usize = 4;
/// pthread.h's PTHREAD_STACK_MIN, and the stack a thread gets if its
/// attributes don't say: 8 MiB, as glibc gives - reserved, so a thread
/// that uses 20 KiB of it takes 20 KiB.
const STACK_MIN: usize = 16 * 1024;
const STACK_DEFAULT: usize = 8 * 1024 * 1024;

type Start = extern "C" fn(*mut c_void) -> *mut c_void;
type Destructor = unsafe extern "C" fn(*mut c_void);

/// A thread, as the C library knows it.
#[repr(C)]
pub struct Thread {
    /// What the thread pointer points at (see the top of this file):
    /// on x86_64 the record's own address, then nothing; on AArch64 two
    /// words for the system.
    block: [usize; 2],
    /// errno, this thread's.
    errno: c_int,
    /// A small number, unique among the program's threads: a mutex
    /// holds its owner's (the main thread is 1).
    id: u32,
    /// 1 while the thread runs; the kernel sets it to 0 at its end.
    running: AtomicU32,
    /// What its start function returned, or pthread_exit was given.
    result: *mut c_void,
    start: Option<Start>,
    argument: *mut c_void,
    /// Its stack, a mapping of its own; null for the main thread, whose
    /// stack is the program's.
    stack: *mut u8,
    /// pthread_setspecific's values, one per key.
    values: [*mut c_void; KEYS_MAX],
    /// C++'s thread_local variables: this thread's copy of each, by
    /// index (see `__emutls_get_address`).
    copies: Vec<*mut c_void>,
}

impl Thread {
    const fn new(id: u32) -> Thread {
        Thread {
            block: [0; 2],
            errno: 0,
            id,
            running: AtomicU32::new(1),
            result: ptr::null_mut(),
            start: None,
            argument: ptr::null_mut(),
            stack: ptr::null_mut(),
            values: [ptr::null_mut(); KEYS_MAX],
            copies: Vec::new(),
        }
    }
}

/// The main thread's record: not made by pthread_create, so it lives
/// here.
struct MainThread(UnsafeCell<Thread>);
// SAFETY: the record is the main thread's; other threads reach it only
// through a pthread_t, as they do every other thread's.
unsafe impl Sync for MainThread {}
static MAIN: MainThread = MainThread(UnsafeCell::new(Thread::new(1)));

static NEXT_ID: AtomicU32 = AtomicU32::new(2);
/// Threads that have not ended, the main thread among them. The last to
/// call pthread_exit ends the program as `exit(0)` would - with atexit's
/// functions and stdio's buffers - which the kernel, ending the process
/// at its last thread, would not do.
static LIVE: AtomicUsize = AtomicUsize::new(1);

/// Every thread's record but the main thread's, for what must reach
/// all of them (a new key starts as null in every thread).
struct Records(Vec<*mut Thread>);
// SAFETY: the pointers are only followed with the list locked.
unsafe impl Send for Records {}
static THREADS: Mutex<Records> = Mutex::new(Records(Vec::new()));
/// Detached threads, freed once they have ended.
static DETACHED: Mutex<Records> = Mutex::new(Records(Vec::new()));

/// Called by `_start` before anything else: the main thread's thread
/// pointer, so that `current` - and errno - work from the first line.
pub(crate) fn init() {
    let main = MAIN.0.get();
    // SAFETY: nothing else runs yet.
    unsafe { (*main).block[0] = main as usize };
    let _ = taaj::set_thread_pointer(main as u64);
}

/// The running thread's record.
pub(crate) fn current() -> *mut Thread {
    let pointer: usize;
    // SAFETY: reads the thread pointer, which `init` and
    // `pthread_create` point at a live record (x86_64: FS:0 holds the
    // record's address).
    #[cfg(target_arch = "x86_64")]
    unsafe {
        core::arch::asm!("mov {}, fs:0", out(reg) pointer, options(nostack, readonly, preserves_flags))
    };
    #[cfg(target_arch = "aarch64")]
    unsafe {
        core::arch::asm!("mrs {}, tpidr_el0", out(reg) pointer, options(nomem, nostack, preserves_flags))
    };
    if pointer == 0 { MAIN.0.get() } else { pointer as *mut Thread }
}

/// errno's home: the running thread's (errno.h:
/// `#define errno (*__errno_location())`).
#[no_mangle]
pub extern "C" fn __errno_location() -> *mut c_int {
    // SAFETY: the running thread's own record, which lives as long as it.
    unsafe { &raw mut (*current()).errno }
}

/// Threads that were detached and have ended: their stacks unmapped,
/// their records freed.
fn free_ended() {
    let ended: Vec<*mut Thread> = {
        let mut detached = DETACHED.lock();
        // SAFETY: a detached record lives until this frees it.
        let (gone, left): (Vec<_>, Vec<_>) =
            detached.0.iter().partition(|&&t| unsafe { (*t).running.load(Ordering::Acquire) } == 0);
        detached.0 = left;
        gone
    };
    for thread in ended {
        // SAFETY: it has ended; nothing uses its stack or record.
        unsafe { free_thread(thread) };
    }
}

/// Unmaps an ended thread's stack and frees its record.
///
/// # Safety
/// The thread must have ended (`running` 0), and nobody else may still
/// use the record.
unsafe fn free_thread(thread: *mut Thread) {
    THREADS.lock().0.retain(|&t| t != thread);
    // SAFETY: the caller's promise.
    let record = unsafe { Box::from_raw(thread) };
    if !record.stack.is_null() {
        let _ = taaj::memory_unmap(record.stack);
    }
}

// ---- threads ------------------------------------------------------------

/// pthread_attr_t: one int. Bit 0: detached. The rest: the stack size
/// in KiB (0: the default).
#[repr(C)]
pub struct Attributes {
    bits: c_int,
}

const DETACHED_BIT: c_int = 1;

impl Attributes {
    fn detached(&self) -> bool {
        self.bits & DETACHED_BIT != 0
    }

    fn stack_size(&self) -> usize {
        match (self.bits as u32 >> 1) as usize * 1024 {
            0 => STACK_DEFAULT,
            size => size,
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn pthread_attr_init(attr: *mut Attributes) -> c_int {
    // SAFETY: the caller's promise.
    unsafe { (*attr).bits = 0 };
    0
}

#[no_mangle]
pub extern "C" fn pthread_attr_destroy(_attr: *mut Attributes) -> c_int {
    0
}

#[no_mangle]
pub unsafe extern "C" fn pthread_attr_setdetachstate(attr: *mut Attributes, state: c_int) -> c_int {
    // SAFETY: the caller's promise.
    let attr = unsafe { &mut *attr };
    match state {
        0 => attr.bits &= !DETACHED_BIT,
        1 => attr.bits |= DETACHED_BIT,
        _ => return EINVAL,
    }
    0
}

#[no_mangle]
pub unsafe extern "C" fn pthread_attr_getdetachstate(attr: *const Attributes, state: *mut c_int) -> c_int {
    // SAFETY: the caller's promise.
    unsafe { *state = c_int::from((*attr).detached()) };
    0
}

#[no_mangle]
pub unsafe extern "C" fn pthread_attr_setstacksize(attr: *mut Attributes, size: usize) -> c_int {
    if size < STACK_MIN || size / 1024 > (c_int::MAX >> 1) as usize {
        return EINVAL;
    }
    // SAFETY: the caller's promise.
    let attr = unsafe { &mut *attr };
    attr.bits = (attr.bits & DETACHED_BIT) | ((size.div_ceil(1024) as c_int) << 1);
    0
}

#[no_mangle]
pub unsafe extern "C" fn pthread_attr_getstacksize(attr: *const Attributes, size: *mut usize) -> c_int {
    // SAFETY: the caller's promise.
    unsafe { *size = (*attr).stack_size() };
    0
}

/// Where every thread pthread_create makes begins.
extern "C" fn begin(thread: *mut Thread) -> ! {
    // SAFETY: pthread_create made the record for this thread.
    let (start, argument) = unsafe { ((*thread).start, (*thread).argument) };
    let result = start.map_or(ptr::null_mut(), |start| start(argument));
    // SAFETY: a thread may always end itself.
    unsafe { pthread_exit(result) }
}

/// `pthread_create(thread, attr, start, argument)`: runs `start(argument)`
/// on a new thread. 0, or EAGAIN (no more threads, or no memory for
/// its stack).
#[no_mangle]
pub unsafe extern "C" fn pthread_create(
    out: *mut usize,
    attr: *const Attributes,
    start: Option<Start>,
    argument: *mut c_void,
) -> c_int {
    free_ended();
    // SAFETY: the caller's promise: null or valid.
    let attr = unsafe { attr.as_ref() };
    let size = attr.map_or(STACK_DEFAULT, Attributes::stack_size).next_multiple_of(4096);
    let Ok(stack) = taaj::memory_reserve(size as u64, true) else { return EAGAIN };

    let mut record = Box::new(Thread::new(NEXT_ID.fetch_add(1, Ordering::Relaxed)));
    record.start = start;
    record.argument = argument;
    record.stack = stack;
    let thread = Box::into_raw(record);
    // SAFETY: just made, not yet shared.
    unsafe { (*thread).block[0] = thread as usize };
    THREADS.lock().0.push(thread);
    LIVE.fetch_add(1, Ordering::Relaxed);
    let detached = attr.is_some_and(Attributes::detached);
    if detached {
        DETACHED.lock().0.push(thread);
    }
    // SAFETY: the caller's promise.
    unsafe { *out = thread as usize };
    let top = stack as u64 + size as u64;
    match taaj::thread_create(begin as *const () as u64, top, thread as u64, thread as u64) {
        Ok(_) => 0,
        Err(_) => {
            if detached {
                DETACHED.lock().0.retain(|&t| t != thread);
            }
            LIVE.fetch_sub(1, Ordering::Relaxed);
            // SAFETY: no thread was started with it.
            unsafe { free_thread(thread) };
            EAGAIN
        }
    }
}

/// `pthread_exit(result)`: ends the running thread, after the
/// destructors of its pthread_setspecific values. The program ends
/// when its last thread does - and at once, with all of them, if any
/// thread calls `exit` or returns from `main`.
#[no_mangle]
pub unsafe extern "C" fn pthread_exit(result: *mut c_void) -> ! {
    let thread = current();
    run_destructors(thread);
    if LIVE.fetch_sub(1, Ordering::AcqRel) == 1 {
        crate::exit(0);
    }
    // SAFETY: the running thread's own record.
    unsafe {
        (*thread).result = result;
        for copy in (*thread).copies.drain(..) {
            crate::memory::free(copy.cast());
        }
        taaj::thread_exit(&(*thread).running)
    }
}

/// `pthread_join(thread, result)`: waits for `thread` to end; what it
/// returned, in `*result` unless null. Its stack and record are freed:
/// a thread is joined once.
#[no_mangle]
pub unsafe extern "C" fn pthread_join(thread: usize, result: *mut *mut c_void) -> c_int {
    let record = thread as *mut Thread;
    if record == current() {
        return EDEADLK;
    }
    // (The main thread may be joined too: its `running` is cleared when
    // it calls pthread_exit.)
    if record.is_null() {
        return ESRCH;
    }
    if DETACHED.lock().0.contains(&record) {
        return EINVAL;
    }
    // SAFETY: a joinable thread's record lives until it is joined.
    let running = unsafe { &(*record).running };
    while running.load(Ordering::Acquire) != 0 {
        let _ = taaj::futex_wait(running, 1, 0);
    }
    if !result.is_null() {
        // SAFETY: the caller's promise; the record is still there.
        unsafe { *result = (*record).result };
    }
    if record != MAIN.0.get() {
        // SAFETY: it has ended, and this is its one join.
        unsafe { free_thread(record) };
    }
    0
}

/// `pthread_detach(thread)`: nobody will join it; its stack and record
/// are freed once it has ended.
#[no_mangle]
pub unsafe extern "C" fn pthread_detach(thread: usize) -> c_int {
    let record = thread as *mut Thread;
    if record.is_null() {
        return ESRCH;
    }
    if record != MAIN.0.get() {
        let mut detached = DETACHED.lock();
        if detached.0.contains(&record) {
            return EINVAL;
        }
        detached.0.push(record);
    }
    free_ended();
    0
}

#[no_mangle]
pub extern "C" fn pthread_self() -> usize {
    current() as usize
}

#[no_mangle]
pub extern "C" fn pthread_equal(a: usize, b: usize) -> c_int {
    c_int::from(a == b)
}

// ---- mutexes ------------------------------------------------------------

/// pthread_mutex_t: [kind | depth << 8][state].
#[repr(C)]
pub struct PthreadMutex {
    kind: c_int,
    state: AtomicU32,
}

const NORMAL: c_int = 0;
const RECURSIVE: c_int = 1;
const ERRORCHECK: c_int = 2;
/// In a mutex's state, with the owner's id: someone may be waiting.
const WAITING: u32 = 1 << 31;

fn me() -> u32 {
    // SAFETY: the running thread's own record.
    unsafe { (*current()).id }
}

impl PthreadMutex {
    fn kind(&self) -> c_int {
        self.kind & 0xFF
    }

    fn depth(&self) -> c_int {
        self.kind >> 8
    }

    fn set_depth(&mut self, depth: c_int) {
        self.kind = self.kind() | (depth << 8);
    }

    fn owner(&self) -> u32 {
        self.state.load(Ordering::Relaxed) & !WAITING
    }

    /// Takes it, waiting as long as it takes (see sync.rs's Lock: the
    /// same three states, with the owner's id for "held").
    fn acquire(&self, me: u32) {
        if self.state.compare_exchange(0, me, Ordering::Acquire, Ordering::Relaxed).is_ok() {
            return;
        }
        loop {
            let state = self.state.load(Ordering::Relaxed);
            if state == 0 {
                // Free: taken as "waited for" - this thread waited, so
                // others may be waiting too, and the unlock must look.
                if self.state.compare_exchange(0, me | WAITING, Ordering::Acquire, Ordering::Relaxed).is_ok() {
                    return;
                }
                continue;
            }
            if state & WAITING == 0
                && self.state.compare_exchange(state, state | WAITING, Ordering::Relaxed, Ordering::Relaxed).is_err()
            {
                continue;
            }
            let _ = taaj::futex_wait(&self.state, state | WAITING, 0);
        }
    }

    fn release(&self) {
        if self.state.swap(0, Ordering::Release) & WAITING != 0 {
            taaj::futex_wake(&self.state, 1);
        }
    }
}

#[repr(C)]
pub struct MutexAttributes {
    kind: c_int,
}

#[no_mangle]
pub unsafe extern "C" fn pthread_mutexattr_init(attr: *mut MutexAttributes) -> c_int {
    // SAFETY: the caller's promise.
    unsafe { (*attr).kind = NORMAL };
    0
}

#[no_mangle]
pub extern "C" fn pthread_mutexattr_destroy(_attr: *mut MutexAttributes) -> c_int {
    0
}

#[no_mangle]
pub unsafe extern "C" fn pthread_mutexattr_settype(attr: *mut MutexAttributes, kind: c_int) -> c_int {
    if !(NORMAL..=ERRORCHECK).contains(&kind) {
        return EINVAL;
    }
    // SAFETY: the caller's promise.
    unsafe { (*attr).kind = kind };
    0
}

#[no_mangle]
pub unsafe extern "C" fn pthread_mutexattr_gettype(attr: *const MutexAttributes, kind: *mut c_int) -> c_int {
    // SAFETY: the caller's promise.
    unsafe { *kind = (*attr).kind };
    0
}

#[no_mangle]
pub unsafe extern "C" fn pthread_mutex_init(mutex: *mut PthreadMutex, attr: *const MutexAttributes) -> c_int {
    // SAFETY: the caller's promise.
    let kind = unsafe { attr.as_ref() }.map_or(NORMAL, |attr| attr.kind);
    // SAFETY: the caller's promise.
    unsafe { mutex.write(PthreadMutex { kind, state: AtomicU32::new(0) }) };
    0
}

#[no_mangle]
pub unsafe extern "C" fn pthread_mutex_destroy(mutex: *mut PthreadMutex) -> c_int {
    // SAFETY: the caller's promise.
    if unsafe { (*mutex).owner() } != 0 { EBUSY } else { 0 }
}

/// `pthread_mutex_lock(mutex)`: waits for it. A normal mutex locked
/// again by its owner is the deadlock POSIX describes: EDEADLK, rather
/// than waiting for ever. A recursive one counts.
#[no_mangle]
pub unsafe extern "C" fn pthread_mutex_lock(mutex: *mut PthreadMutex) -> c_int {
    // SAFETY: the caller's promise.
    let mutex = unsafe { &mut *mutex };
    let me = me();
    if mutex.owner() == me {
        return match mutex.kind() {
            RECURSIVE if mutex.depth() < 0xFF_FFFF => {
                mutex.set_depth(mutex.depth() + 1);
                0
            }
            RECURSIVE => EAGAIN,
            _ => EDEADLK,
        };
    }
    mutex.acquire(me);
    0
}

#[no_mangle]
pub unsafe extern "C" fn pthread_mutex_trylock(mutex: *mut PthreadMutex) -> c_int {
    // SAFETY: the caller's promise.
    let mutex = unsafe { &mut *mutex };
    let me = me();
    if mutex.owner() == me && mutex.kind() == RECURSIVE {
        // SAFETY: the caller's promise, passed on.
        return unsafe { pthread_mutex_lock(mutex as *mut PthreadMutex) };
    }
    match mutex.state.compare_exchange(0, me, Ordering::Acquire, Ordering::Relaxed) {
        Ok(_) => 0,
        Err(_) => EBUSY,
    }
}

/// `pthread_mutex_unlock(mutex)`: EPERM unless the running thread holds
/// it.
#[no_mangle]
pub unsafe extern "C" fn pthread_mutex_unlock(mutex: *mut PthreadMutex) -> c_int {
    // SAFETY: the caller's promise.
    let mutex = unsafe { &mut *mutex };
    if mutex.owner() != me() {
        return EPERM;
    }
    if mutex.depth() > 0 {
        mutex.set_depth(mutex.depth() - 1);
        return 0;
    }
    mutex.release();
    0
}

// ---- condition variables ------------------------------------------------

/// pthread_cond_t: a sequence number, moved on by every signal and
/// broadcast. A waiter reads it, lets the mutex go, and sleeps while it
/// is unchanged: a signal that comes in between changes it, and the
/// sleep doesn't begin.
#[repr(C)]
pub struct Condition {
    sequence: AtomicU32,
}

#[no_mangle]
pub extern "C" fn pthread_condattr_init(_attr: *mut c_int) -> c_int {
    0
}

#[no_mangle]
pub extern "C" fn pthread_condattr_destroy(_attr: *mut c_int) -> c_int {
    0
}

#[no_mangle]
pub unsafe extern "C" fn pthread_cond_init(cond: *mut Condition, _attr: *const c_int) -> c_int {
    // SAFETY: the caller's promise.
    unsafe { cond.write(Condition { sequence: AtomicU32::new(0) }) };
    0
}

#[no_mangle]
pub extern "C" fn pthread_cond_destroy(_cond: *mut Condition) -> c_int {
    0
}

#[no_mangle]
pub unsafe extern "C" fn pthread_cond_signal(cond: *mut Condition) -> c_int {
    // SAFETY: the caller's promise.
    let cond = unsafe { &*cond };
    cond.sequence.fetch_add(1, Ordering::Release);
    taaj::futex_wake(&cond.sequence, 1);
    0
}

#[no_mangle]
pub unsafe extern "C" fn pthread_cond_broadcast(cond: *mut Condition) -> c_int {
    // SAFETY: the caller's promise.
    let cond = unsafe { &*cond };
    cond.sequence.fetch_add(1, Ordering::Release);
    taaj::futex_wake(&cond.sequence, u32::MAX);
    0
}

/// The wait itself: `deadline` in ticks, 0 for none. The mutex is let
/// go however deep a recursive one is held, and taken back as deep.
fn wait(cond: &Condition, mutex: &mut PthreadMutex, deadline: u64) -> c_int {
    let me = me();
    if mutex.owner() != me {
        return EPERM;
    }
    let sequence = cond.sequence.load(Ordering::Acquire);
    let depth = mutex.depth();
    mutex.set_depth(0);
    mutex.release();
    let waited = taaj::futex_wait(&cond.sequence, sequence, deadline);
    mutex.acquire(me);
    mutex.set_depth(depth);
    match waited {
        Err(taaj::Error::TimedOut) => ETIMEDOUT,
        _ => 0, // woken, changed already, or woken for nothing: the caller looks again
    }
}

#[no_mangle]
pub unsafe extern "C" fn pthread_cond_wait(cond: *mut Condition, mutex: *mut PthreadMutex) -> c_int {
    // SAFETY: the caller's promise.
    unsafe { wait(&*cond, &mut *mutex, 0) }
}

#[repr(C)]
pub struct Timespec {
    tv_sec: c_long,
    tv_nsec: c_long,
}

/// A CLOCK_REALTIME time as the kernel's tick count, which counts from
/// boot: `boot` is the time of day boot was at, in nanoseconds since
/// 1970. Tick n is when the time since boot reaches n ticks, so the
/// deadline is rounded up - never early. At least 1: 0 is "no deadline"
/// to the kernel.
fn ticks_at(time: &Timespec, boot: u64) -> Option<u64> {
    if !(0..1_000_000_000).contains(&time.tv_nsec) {
        return None;
    }
    let at = u64::try_from(time.tv_sec).unwrap_or(0).saturating_mul(1_000_000_000).saturating_add(time.tv_nsec as u64);
    let tick = 1_000_000_000 / abi::TICKS_PER_SECOND;
    Some(at.saturating_sub(boot).div_ceil(tick).max(1))
}

/// The time of day at boot, in nanoseconds since 1970. Read as the time
/// of day first, then the time since boot: the second read is a little
/// later, so this comes out a little early - and a deadline worked out
/// from it a little late, never early.
fn boot_time() -> u64 {
    let now = taaj::realtime_ns();
    now.saturating_sub(taaj::monotonic_ns())
}

#[no_mangle]
pub unsafe extern "C" fn pthread_cond_timedwait(
    cond: *mut Condition,
    mutex: *mut PthreadMutex,
    deadline: *const Timespec,
) -> c_int {
    // SAFETY: the caller's promise.
    let Some(ticks) = ticks_at(unsafe { &*deadline }, boot_time()) else { return EINVAL };
    if ticks <= taaj::ticks() {
        // SAFETY: the caller's promise.
        return if unsafe { (*mutex).owner() } == me() { ETIMEDOUT } else { EPERM };
    }
    // SAFETY: the caller's promise.
    unsafe { wait(&*cond, &mut *mutex, ticks) }
}

// ---- read-write locks ---------------------------------------------------

/// pthread_rwlock_t: [readers, or WRITER][sequence]. Whoever can't have
/// it sleeps on the sequence, which every unlock that leaves it free
/// moves on. Readers are let in while other readers hold it, even with
/// a writer waiting: a writer can wait long under many readers.
#[repr(C)]
pub struct ReadWriteLock {
    state: AtomicU32,
    sequence: AtomicU32,
}

const WRITER: u32 = u32::MAX;

impl ReadWriteLock {
    fn try_read(&self) -> bool {
        let state = self.state.load(Ordering::Relaxed);
        state < WRITER - 1
            && self.state.compare_exchange(state, state + 1, Ordering::Acquire, Ordering::Relaxed).is_ok()
    }

    fn try_write(&self) -> bool {
        self.state.compare_exchange(0, WRITER, Ordering::Acquire, Ordering::Relaxed).is_ok()
    }

    fn wait_while(&self, busy: impl Fn(u32) -> bool) {
        let sequence = self.sequence.load(Ordering::Acquire);
        if busy(self.state.load(Ordering::Acquire)) {
            let _ = taaj::futex_wait(&self.sequence, sequence, 0);
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn pthread_rwlock_init(lock: *mut ReadWriteLock, _attr: *const c_int) -> c_int {
    // SAFETY: the caller's promise.
    unsafe { lock.write(ReadWriteLock { state: AtomicU32::new(0), sequence: AtomicU32::new(0) }) };
    0
}

#[no_mangle]
pub unsafe extern "C" fn pthread_rwlock_destroy(lock: *mut ReadWriteLock) -> c_int {
    // SAFETY: the caller's promise.
    if unsafe { (*lock).state.load(Ordering::Relaxed) } != 0 { EBUSY } else { 0 }
}

#[no_mangle]
pub unsafe extern "C" fn pthread_rwlock_rdlock(lock: *mut ReadWriteLock) -> c_int {
    // SAFETY: the caller's promise.
    let lock = unsafe { &*lock };
    while !lock.try_read() {
        lock.wait_while(|state| state == WRITER);
    }
    0
}

#[no_mangle]
pub unsafe extern "C" fn pthread_rwlock_tryrdlock(lock: *mut ReadWriteLock) -> c_int {
    // SAFETY: the caller's promise.
    if unsafe { (*lock).try_read() } { 0 } else { EBUSY }
}

#[no_mangle]
pub unsafe extern "C" fn pthread_rwlock_wrlock(lock: *mut ReadWriteLock) -> c_int {
    // SAFETY: the caller's promise.
    let lock = unsafe { &*lock };
    while !lock.try_write() {
        lock.wait_while(|state| state != 0);
    }
    0
}

#[no_mangle]
pub unsafe extern "C" fn pthread_rwlock_trywrlock(lock: *mut ReadWriteLock) -> c_int {
    // SAFETY: the caller's promise.
    if unsafe { (*lock).try_write() } { 0 } else { EBUSY }
}

#[no_mangle]
pub unsafe extern "C" fn pthread_rwlock_unlock(lock: *mut ReadWriteLock) -> c_int {
    // SAFETY: the caller's promise.
    let lock = unsafe { &*lock };
    let free = match lock.state.load(Ordering::Relaxed) {
        0 => return EPERM,
        WRITER => {
            lock.state.store(0, Ordering::Release);
            true
        }
        _ => lock.state.fetch_sub(1, Ordering::Release) == 1,
    };
    if free {
        lock.sequence.fetch_add(1, Ordering::Release);
        taaj::futex_wake(&lock.sequence, u32::MAX);
    }
    0
}

// ---- once ---------------------------------------------------------------

/// `pthread_once(once, function)`: runs `function` the first time any
/// thread gets here; the others wait until it has returned.
#[no_mangle]
pub unsafe extern "C" fn pthread_once(once: *mut c_int, function: extern "C" fn()) -> c_int {
    // SAFETY: the caller's promise: an int, used for nothing else.
    let state = unsafe { AtomicU32::from_ptr(once.cast::<u32>()) };
    match state.compare_exchange(0, 1, Ordering::Acquire, Ordering::Acquire) {
        Ok(_) => {
            function();
            state.store(2, Ordering::Release);
            taaj::futex_wake(state, u32::MAX);
        }
        Err(_) => {
            while state.load(Ordering::Acquire) == 1 {
                let _ = taaj::futex_wait(state, 1, 0);
            }
        }
    }
    0
}

// ---- keys ---------------------------------------------------------------

/// The keys in use, and each one's destructor.
static KEYS: Mutex<[Option<Option<Destructor>>; KEYS_MAX]> = Mutex::new([None; KEYS_MAX]);

#[no_mangle]
pub unsafe extern "C" fn pthread_key_create(key: *mut u32, destructor: Option<Destructor>) -> c_int {
    let mut keys = KEYS.lock();
    let Some(free) = keys.iter().position(Option::is_none) else { return EAGAIN };
    keys[free] = Some(destructor);
    // A key's value starts as null in every thread - including any a
    // deleted key of the same number left behind.
    // SAFETY: the records are live while listed, and main's always is.
    unsafe {
        (*MAIN.0.get()).values[free] = ptr::null_mut();
        for &thread in &THREADS.lock().0 {
            (*thread).values[free] = ptr::null_mut();
        }
        *key = free as u32;
    }
    0
}

#[no_mangle]
pub extern "C" fn pthread_key_delete(key: u32) -> c_int {
    match KEYS.lock().get_mut(key as usize) {
        Some(slot @ Some(_)) => {
            *slot = None;
            0
        }
        _ => EINVAL,
    }
}

#[no_mangle]
pub extern "C" fn pthread_getspecific(key: u32) -> *mut c_void {
    // SAFETY: the running thread's own record.
    unsafe { (*current()).values.get(key as usize).copied().unwrap_or(ptr::null_mut()) }
}

#[no_mangle]
pub extern "C" fn pthread_setspecific(key: u32, value: *const c_void) -> c_int {
    // SAFETY: the running thread's own record.
    match unsafe { (*current()).values.get_mut(key as usize) } {
        Some(slot) => {
            *slot = value.cast_mut();
            0
        }
        None => EINVAL,
    }
}

/// At a thread's end: each value that is not null, with a destructor,
/// is set to null and given to it - again while destructors leave new
/// values, up to DESTRUCTOR_ROUNDS times.
fn run_destructors(thread: *mut Thread) {
    for _ in 0..DESTRUCTOR_ROUNDS {
        let mut ran = false;
        for key in 0..KEYS_MAX {
            let destructor = KEYS.lock()[key].flatten();
            // SAFETY: the running thread's own record.
            let value = unsafe { core::mem::replace(&mut (*thread).values[key], ptr::null_mut()) };
            if let (Some(destructor), false) = (destructor, value.is_null()) {
                // SAFETY: the program gave the destructor for exactly this.
                unsafe { destructor(value) };
                ran = true;
            }
        }
        if !ran {
            break;
        }
    }
}

// ---- C++'s thread_local -------------------------------------------------

/// The record the compiler makes for each thread_local variable when
/// TheTaaJ's C++ is compiled with -femulated-tls.
#[repr(C)]
pub struct EmutlsControl {
    size: usize,
    align: usize,
    /// The variable's index, plus one, once it has one (0 before).
    index: AtomicUsize,
    /// The bytes it starts with, or null for zeros.
    value: *const u8,
}

static NEXT_INDEX: AtomicUsize = AtomicUsize::new(1);

/// The running thread's copy of the variable `control` describes: made
/// the first time this thread asks, from the bytes it starts with.
#[no_mangle]
pub unsafe extern "C" fn __emutls_get_address(control: *mut EmutlsControl) -> *mut c_void {
    // SAFETY: the compiler's record.
    let control = unsafe { &*control };
    let mut index = control.index.load(Ordering::Acquire);
    if index == 0 {
        let fresh = NEXT_INDEX.fetch_add(1, Ordering::Relaxed);
        index = match control.index.compare_exchange(0, fresh, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => fresh,
            Err(theirs) => theirs, // another thread gave it one first
        };
    }
    // SAFETY: the running thread's own record.
    let copies = unsafe { &mut (*current()).copies };
    if copies.len() < index {
        copies.resize(index, ptr::null_mut());
    }
    if copies[index - 1].is_null() {
        let align = control.align.max(core::mem::size_of::<usize>());
        let size = control.size.next_multiple_of(align);
        let copy = crate::memory::aligned_alloc(align, size);
        if copy.is_null() {
            return ptr::null_mut(); // as the one-thread version did: the program faults on it
        }
        // SAFETY: just allocated, `size` >= control.size bytes.
        unsafe {
            if control.value.is_null() {
                ptr::write_bytes(copy.cast::<u8>(), 0, control.size);
            } else {
                ptr::copy_nonoverlapping(control.value, copy.cast::<u8>(), control.size);
            }
        }
        copies[index - 1] = copy;
    }
    copies[index - 1]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deadlines_round_up_to_ticks() {
        let at = |s, ns| ticks_at(&Timespec { tv_sec: s, tv_nsec: ns }, 0);
        assert_eq!(at(2, 0), Some(200));
        assert_eq!(at(2, 1), Some(201), "never early");
        assert_eq!(at(2, 10_000_000), Some(201));
        assert_eq!(at(0, 0), Some(1), "0 means no deadline to the kernel");
        assert_eq!(at(1, 1_000_000_000), None);
        assert_eq!(at(1, -1), None);
    }

    #[test]
    fn deadlines_count_from_boot() {
        // Booted at 2026-11-25 13:45:30.25 UTC.
        let boot = 1_795_614_330 * 1_000_000_000 + 250_000_000;
        let at = |s, ns| ticks_at(&Timespec { tv_sec: s, tv_nsec: ns }, boot);
        assert_eq!(at(1_795_614_331, 250_000_000), Some(100), "a second after boot");
        assert_eq!(at(1_795_614_331, 250_000_001), Some(101));
        assert_eq!(at(1_795_614_330, 0), Some(1), "before boot: due at once");
        assert_eq!(at(0, 0), Some(1));
    }
}

#[allow(dead_code)]
fn _sizes() {
    // The sizes pthread.h gives these types.
    const _: () = assert!(core::mem::size_of::<PthreadMutex>() == 8);
    const _: () = assert!(core::mem::size_of::<Condition>() == 4);
    const _: () = assert!(core::mem::size_of::<ReadWriteLock>() == 8);
    const _: () = assert!(core::mem::size_of::<Attributes>() == 4);
}
