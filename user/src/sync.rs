//! Locks for a program with threads, built on the kernel's futexes.
//!
//! `Lock` is the classic three-state futex lock (Ulrich Drepper's
//! "Futexes Are Tricky", mutex 2): a u32 that is 0 when free, 1 when
//! held, and 2 when held with someone perhaps waiting. Taking a free
//! lock and giving back one nobody waits for are single atomic
//! instructions - no system call. Only a thread that finds the lock
//! held asks the kernel to sleep (FUTEX_WAIT, "while it is still 2"),
//! and only an unlock that finds a 2 asks it to wake one.
//!
//! The memory allocator, the C library's tables and its pthread mutexes
//! all use it.

use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicU32, Ordering};

const FREE: u32 = 0;
const HELD: u32 = 1;
const WAITED_FOR: u32 = 2;

/// A lock with nothing in it: see the top of this file.
pub struct Lock {
    state: AtomicU32,
}

impl Lock {
    pub const fn new() -> Lock {
        Lock { state: AtomicU32::new(FREE) }
    }

    pub fn lock(&self) {
        if self.state.compare_exchange(FREE, HELD, Ordering::Acquire, Ordering::Relaxed).is_ok() {
            return;
        }
        // Held. From here on it is marked "waited for", even if this
        // thread ends up taking it at once: an unlock then wakes one
        // thread too many, which only costs it a look.
        while self.state.swap(WAITED_FOR, Ordering::Acquire) != FREE {
            let _ = crate::futex_wait(&self.state, WAITED_FOR, 0);
        }
    }

    /// Takes the lock if it is free; whether it did.
    pub fn try_lock(&self) -> bool {
        self.state.compare_exchange(FREE, HELD, Ordering::Acquire, Ordering::Relaxed).is_ok()
    }

    /// Gives the lock back. Only the thread that took it may.
    pub fn unlock(&self) {
        if self.state.swap(FREE, Ordering::Release) == WAITED_FOR {
            crate::futex_wake(&self.state, 1);
        }
    }
}

impl Default for Lock {
    fn default() -> Lock {
        Lock::new()
    }
}

/// A value that one thread at a time may reach.
pub struct Mutex<T> {
    lock: Lock,
    value: UnsafeCell<T>,
}

// SAFETY: the value is reached only through a guard, and a guard only
// while the lock is held; sending the value between threads is what
// the lock is for, so T must be Send.
unsafe impl<T: Send> Sync for Mutex<T> {}
// SAFETY: the mutex owns its value; moving it moves the value, which is
// sound whenever T may move between threads.
unsafe impl<T: Send> Send for Mutex<T> {}

impl<T> Mutex<T> {
    pub const fn new(value: T) -> Mutex<T> {
        Mutex { lock: Lock::new(), value: UnsafeCell::new(value) }
    }

    /// Waits for the lock; the value, until the guard goes.
    pub fn lock(&self) -> MutexGuard<'_, T> {
        self.lock.lock();
        MutexGuard { mutex: self }
    }

    /// The value, if the lock is free now; None without waiting if not.
    pub fn try_lock(&self) -> Option<MutexGuard<'_, T>> {
        self.lock.try_lock().then_some(MutexGuard { mutex: self })
    }
}

/// The value of a locked Mutex. Dropping it unlocks.
pub struct MutexGuard<'a, T> {
    mutex: &'a Mutex<T>,
}

impl<T> Deref for MutexGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        // SAFETY: the lock is held while the guard lives.
        unsafe { &*self.mutex.value.get() }
    }
}

impl<T> DerefMut for MutexGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: as above, and the guard is borrowed mutably.
        unsafe { &mut *self.mutex.value.get() }
    }
}

impl<T> Drop for MutexGuard<'_, T> {
    fn drop(&mut self) {
        self.mutex.lock.unlock();
    }
}
