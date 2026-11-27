//! Scheduling: `nice`, `getpriority`/`setpriority`, the `sched_*` calls
//! and their `pthread_*` forms (MaK-WSL/sched's J80, with K20; journal
//! entry 124), over the kernel's SCHED_SET and SCHED_GET.
//!
//! A priority is `nice`'s number: -20 the most important, 19 the least,
//! 0 the default. The kernel keeps one per thread, and a process's for
//! its new threads; priorities are strict (see the kernel's sched.rs),
//! so a busy program at a better priority leaves nothing for one at a
//! worse. How far up a program may go is its ceiling: the default (0)
//! for everything the shell or posix_spawn starts. So a program may make
//! itself less important (`nice 10`), and not more (`nice -5` is EPERM).
//!
//! Who: 0 or the program's own number is the program itself; a child it
//! started and has not waited for is that child (its process handle,
//! which carries the right). Anyone else is ESRCH: TheTaaJ gives a
//! program no way to reach a process it was not handed.
//!
//! The scheduling policy is always SCHED_OTHER. SCHED_FIFO and SCHED_RR
//! (fixed real-time priorities) are classes the kernel does not have yet
//! (K99): asking for them is EPERM, as for a program without the right.

use crate::descriptors::fail;
use crate::errno::{EACCES, EINVAL, EPERM, ESRCH};
use core::ffi::c_int;
use taaj::Error;

const PRIO_PROCESS: c_int = 0;
const SCHED_OTHER: c_int = 0;
const SCHED_FIFO: c_int = 1;
const SCHED_RR: c_int = 2;

/// `struct sched_param`.
#[repr(C)]
pub struct SchedParam {
    sched_priority: c_int,
}

/// `cpu_set_t`: one bit per CPU, CPU_SETSIZE (64) of them.
#[repr(C)]
pub struct CpuSet {
    bits: [u64; 1],
}

/// The kernel's target for `who`: this process (0, or its own number),
/// or a child's process handle; None if `who` is nobody this program can
/// reach.
fn process(who: c_int) -> Option<u64> {
    if who == 0 || who == crate::system::getpid() {
        Some(0)
    } else {
        crate::spawn::child_process(who)
    }
}

/// `getpriority(which, who)`: the process's priority, -20 to 19. As the
/// value can be -1, a caller tells an error by errno: set it to 0 first.
#[no_mangle]
pub extern "C" fn getpriority(which: c_int, who: c_int) -> c_int {
    if which != PRIO_PROCESS {
        return fail(EINVAL); // no process groups or users here
    }
    let Some(target) = process(who) else { return fail(ESRCH) };
    match taaj::sched_get(abi::sched::PROCESS, target, abi::sched::PRIORITY) {
        Ok(level) => abi::sched::priority(level) as c_int,
        Err(_) => fail(ESRCH),
    }
}

/// `setpriority(which, who, priority)`: every thread of the process, and
/// those it makes later. EACCES for a priority above its ceiling.
#[no_mangle]
pub extern "C" fn setpriority(which: c_int, who: c_int, priority: c_int) -> c_int {
    if which != PRIO_PROCESS {
        return fail(EINVAL);
    }
    let Some(target) = process(who) else { return fail(ESRCH) };
    let level = abi::sched::level(i64::from(priority));
    match taaj::sched_set(abi::sched::PROCESS, target, abi::sched::PRIORITY, level) {
        Ok(()) => 0,
        Err(Error::AccessDenied) => fail(EACCES),
        Err(_) => fail(ESRCH),
    }
}

/// `nice(increment)`: the process's priority moved by `increment`
/// (positive: less important), and the new one returned; -1 with EPERM
/// past the ceiling. Clamped to -20..19, as Linux does.
#[no_mangle]
pub extern "C" fn nice(increment: c_int) -> c_int {
    let Ok(level) = taaj::sched_get(abi::sched::PROCESS, 0, abi::sched::PRIORITY) else { return fail(EPERM) };
    let wanted = (abi::sched::priority(level) + i64::from(increment)).clamp(abi::sched::HIGHEST, abi::sched::LOWEST);
    match taaj::sched_set(abi::sched::PROCESS, 0, abi::sched::PRIORITY, abi::sched::level(wanted)) {
        Ok(()) => wanted as c_int,
        Err(_) => fail(EPERM),
    }
}

/// Reads the affinity of a thread (`target` THREAD) or process into
/// `set`; `bytes` is its size.
///
/// # Safety
/// `set` must be valid for `bytes` bytes.
unsafe fn get_affinity(target: u64, id: u64, bytes: usize, set: *mut CpuSet) -> c_int {
    if bytes < core::mem::size_of::<CpuSet>() || set.is_null() {
        return EINVAL;
    }
    match taaj::sched_get(target, id, abi::sched::AFFINITY) {
        // SAFETY: the caller's promise.
        Ok(mask) => unsafe {
            core::ptr::write_bytes(set.cast::<u8>(), 0, bytes);
            (*set).bits[0] = mask;
            0
        },
        Err(_) => ESRCH,
    }
}

/// Sets it.
///
/// # Safety
/// `set` must be valid for `bytes` bytes.
unsafe fn set_affinity(target: u64, id: u64, bytes: usize, set: *const CpuSet) -> c_int {
    if bytes < core::mem::size_of::<CpuSet>() || set.is_null() {
        return EINVAL;
    }
    // SAFETY: the caller's promise.
    let mask = unsafe { (*set).bits[0] };
    match taaj::sched_set(target, id, abi::sched::AFFINITY, mask) {
        Ok(()) => 0,
        Err(Error::InvalidArgument) => EINVAL, // no CPU there
        Err(_) => ESRCH,
    }
}

/// `sched_setaffinity(pid, size, set)`: the CPUs it may run on. As on
/// Linux, 0 is the calling thread; a child is every thread of the child.
///
/// # Safety
/// `set` must be valid for `size` bytes.
#[no_mangle]
pub unsafe extern "C" fn sched_setaffinity(pid: c_int, size: usize, set: *const CpuSet) -> c_int {
    let (target, id) = if pid == 0 {
        (abi::sched::THREAD, 0)
    } else {
        match process(pid) {
            Some(handle) => (abi::sched::PROCESS, handle),
            None => return fail(ESRCH),
        }
    };
    // SAFETY: the caller's promise.
    match unsafe { set_affinity(target, id, size, set) } {
        0 => 0,
        error => fail(error),
    }
}

/// `sched_getaffinity(pid, size, set)`.
///
/// # Safety
/// `set` must be valid for `size` bytes.
#[no_mangle]
pub unsafe extern "C" fn sched_getaffinity(pid: c_int, size: usize, set: *mut CpuSet) -> c_int {
    let (target, id) = if pid == 0 {
        (abi::sched::THREAD, 0)
    } else {
        match process(pid) {
            Some(handle) => (abi::sched::PROCESS, handle),
            None => return fail(ESRCH),
        }
    };
    // SAFETY: the caller's promise.
    match unsafe { get_affinity(target, id, size, set) } {
        0 => 0,
        error => fail(error),
    }
}

/// `pthread_setaffinity_np(thread, size, set)`: one thread's CPUs. An
/// error number, not errno, as pthreads do.
///
/// # Safety
/// `set` must be valid for `size` bytes.
#[no_mangle]
pub unsafe extern "C" fn pthread_setaffinity_np(thread: usize, size: usize, set: *const CpuSet) -> c_int {
    let id = if thread == crate::threads::pthread_self() { Some(0) } else { crate::threads::kernel_id(thread) };
    let Some(id) = id else { return ESRCH };
    // SAFETY: the caller's promise.
    unsafe { set_affinity(abi::sched::THREAD, id, size, set) }
}

/// `pthread_getaffinity_np(thread, size, set)`.
///
/// # Safety
/// `set` must be valid for `size` bytes.
#[no_mangle]
pub unsafe extern "C" fn pthread_getaffinity_np(thread: usize, size: usize, set: *mut CpuSet) -> c_int {
    let id = if thread == crate::threads::pthread_self() { Some(0) } else { crate::threads::kernel_id(thread) };
    let Some(id) = id else { return ESRCH };
    // SAFETY: the caller's promise.
    unsafe { get_affinity(abi::sched::THREAD, id, size, set) }
}

/// The policies: SCHED_OTHER only, its one priority 0.
fn check_policy(policy: c_int, param: *const SchedParam) -> c_int {
    match policy {
        // SAFETY: a null param is the caller's mistake, said as EINVAL.
        SCHED_OTHER if !param.is_null() && unsafe { (*param).sched_priority } == 0 => 0,
        SCHED_OTHER => EINVAL,
        SCHED_FIFO | SCHED_RR => EPERM, // no real-time class yet (K99)
        _ => EINVAL,
    }
}

/// `sched_setscheduler(pid, policy, param)`: SCHED_OTHER with priority
/// 0, which every thread already has; FIFO and RR are EPERM.
#[no_mangle]
pub extern "C" fn sched_setscheduler(pid: c_int, policy: c_int, param: *const SchedParam) -> c_int {
    if pid != 0 && process(pid).is_none() {
        return fail(ESRCH);
    }
    match check_policy(policy, param) {
        0 => 0,
        error => fail(error),
    }
}

/// `sched_getscheduler(pid)`: always SCHED_OTHER.
#[no_mangle]
pub extern "C" fn sched_getscheduler(pid: c_int) -> c_int {
    if pid != 0 && process(pid).is_none() {
        return fail(ESRCH);
    }
    SCHED_OTHER
}

/// `sched_setparam(pid, param)`: priority 0 is all SCHED_OTHER has.
#[no_mangle]
pub extern "C" fn sched_setparam(pid: c_int, param: *const SchedParam) -> c_int {
    sched_setscheduler(pid, SCHED_OTHER, param)
}

/// `sched_getparam(pid, param)`: priority 0.
///
/// # Safety
/// `param` must be valid for writing.
#[no_mangle]
pub unsafe extern "C" fn sched_getparam(pid: c_int, param: *mut SchedParam) -> c_int {
    if pid != 0 && process(pid).is_none() {
        return fail(ESRCH);
    }
    if param.is_null() {
        return fail(EINVAL);
    }
    // SAFETY: the caller's promise.
    unsafe { (*param).sched_priority = 0 };
    0
}

/// `sched_get_priority_max(policy)` and `_min`: 0 for SCHED_OTHER (its
/// static priority; `nice` is the dynamic one), Linux's 1..99 for the
/// real-time policies a program may name but not yet use.
#[no_mangle]
pub extern "C" fn sched_get_priority_max(policy: c_int) -> c_int {
    match policy {
        SCHED_OTHER => 0,
        SCHED_FIFO | SCHED_RR => 99,
        _ => fail(EINVAL),
    }
}

#[no_mangle]
pub extern "C" fn sched_get_priority_min(policy: c_int) -> c_int {
    match policy {
        SCHED_OTHER => 0,
        SCHED_FIFO | SCHED_RR => 1,
        _ => fail(EINVAL),
    }
}

/// `pthread_setschedparam(thread, policy, param)`: as sched_setscheduler,
/// for one thread; an error number, not errno.
#[no_mangle]
pub extern "C" fn pthread_setschedparam(thread: usize, policy: c_int, param: *const SchedParam) -> c_int {
    if thread != crate::threads::pthread_self() && crate::threads::kernel_id(thread).is_none() {
        return ESRCH;
    }
    check_policy(policy, param)
}

/// `pthread_getschedparam(thread, policy, param)`: SCHED_OTHER, 0.
///
/// # Safety
/// `policy` and `param` must be valid for writing.
#[no_mangle]
pub unsafe extern "C" fn pthread_getschedparam(thread: usize, policy: *mut c_int, param: *mut SchedParam) -> c_int {
    if thread != crate::threads::pthread_self() && crate::threads::kernel_id(thread).is_none() {
        return ESRCH;
    }
    if policy.is_null() || param.is_null() {
        return EINVAL;
    }
    // SAFETY: the caller's promise.
    unsafe {
        *policy = SCHED_OTHER;
        (*param).sched_priority = 0;
    }
    0
}
