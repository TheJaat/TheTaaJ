//! Locks.
//!
//! Rust will not let two places hold a `&mut` to the same data, and a
//! `static` can be reached from anywhere - so a global that can change
//! has to go behind something that hands out access one holder at a
//! time. That is a lock.
//!
//! A spinlock is the simplest: one atomic flag, and a waiter loops until
//! it is clear. Right for short critical sections in a kernel, where
//! there is no scheduler to sleep on yet.
//!
//! Interrupts are not touched here - this module knows no CPU. A lock an
//! interrupt handler can also take must disable interrupts while held,
//! or the handler can spin forever on a lock its own CPU holds; the
//! kernel wraps this one as `IrqLock` for that (irqlock.rs).

use core::cell::UnsafeCell;
use core::hint;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, Ordering};

pub struct SpinLock<T> {
    locked: AtomicBool,
    value: UnsafeCell<T>,
}

// SAFETY: the lock gives out access to `value` to one holder at a time,
// so sharing the lock between threads is sound whenever moving the value
// between threads is.
unsafe impl<T: Send> Sync for SpinLock<T> {}
unsafe impl<T: Send> Send for SpinLock<T> {}

impl<T> SpinLock<T> {
    pub const fn new(value: T) -> SpinLock<T> {
        SpinLock { locked: AtomicBool::new(false), value: UnsafeCell::new(value) }
    }

    /// Waits until the lock is free, then takes it. Dropping the guard
    /// releases it - there is no way to forget to unlock.
    pub fn lock(&self) -> SpinLockGuard<'_, T> {
        loop {
            if let Some(guard) = self.try_lock() {
                return guard;
            }
            // Wait with plain reads until it looks free: retrying the
            // atomic swap in a loop would fight the holder for the cache
            // line. `spin_loop` is the `pause` instruction.
            while self.locked.load(Ordering::Relaxed) {
                hint::spin_loop();
            }
        }
    }

    /// Releases the lock whoever holds it.
    ///
    /// # Safety
    /// Only for a dying kernel that must print one last message even if
    /// the console's lock was held when it failed. The holder's guard
    /// must never be used again.
    pub unsafe fn force_unlock(&self) {
        self.locked.store(false, Ordering::Release);
    }

    pub fn try_lock(&self) -> Option<SpinLockGuard<'_, T>> {
        // Acquire: everything the previous holder wrote before releasing
        // is visible to us once we hold the lock.
        self.locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .ok()
            .map(|_| SpinLockGuard { lock: self })
    }
}

pub struct SpinLockGuard<'a, T> {
    lock: &'a SpinLock<T>,
}

impl<T> Deref for SpinLockGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        // SAFETY: holding the guard means holding the lock.
        unsafe { &*self.lock.value.get() }
    }
}

impl<T> DerefMut for SpinLockGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: as above, and `&mut self` makes this the only access.
        unsafe { &mut *self.lock.value.get() }
    }
}

impl<T> Drop for SpinLockGuard<'_, T> {
    fn drop(&mut self) {
        // Release: our writes are visible to the next holder.
        self.lock.locked.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn guards_exclude_each_other() {
        let lock = SpinLock::new(5);
        let guard = lock.lock();
        assert!(lock.try_lock().is_none());
        drop(guard);
        *lock.try_lock().unwrap() += 1;
        assert_eq!(*lock.lock(), 6);
    }

    /// Real threads hammering one counter: without mutual exclusion the
    /// unsynchronised `+= 1`s would lose updates.
    #[test]
    fn no_lost_updates_under_contention() {
        let counter = Arc::new(SpinLock::new(0u64));
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let counter = Arc::clone(&counter);
                thread::spawn(move || {
                    for _ in 0..20_000 {
                        *counter.lock() += 1;
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        assert_eq!(*counter.lock(), 8 * 20_000);
    }
}
