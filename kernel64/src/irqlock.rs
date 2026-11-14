//! `IrqLock`: a spinlock that also keeps interrupts off while held.
//!
//! Why: suppose the main loop holds the heap lock and the timer fires.
//! If the timer handler then allocates, it spins on the heap lock - but
//! the holder cannot run again until the handler returns. The CPU hangs
//! forever. Disabling interrupts for as long as the lock is held makes
//! that impossible on this CPU; the spin part still protects against
//! other CPUs, once there are any.
//!
//! Interrupts are restored to what they were, not blindly enabled, so
//! these nest: taking a second IrqLock inside the first leaves them off
//! until the outer one is released.

use core::mem::ManuallyDrop;
use core::ops::{Deref, DerefMut};

use kernel64::sync::{SpinLock, SpinLockGuard};

use crate::arch;

pub struct IrqLock<T> {
    inner: SpinLock<T>,
}

impl<T> IrqLock<T> {
    pub const fn new(value: T) -> IrqLock<T> {
        IrqLock { inner: SpinLock::new(value) }
    }

    pub fn lock(&self) -> IrqLockGuard<'_, T> {
        // Interrupts off FIRST: otherwise one could arrive between taking
        // the lock and disabling them - the very window this closes.
        let were_enabled = arch::disable_interrupts();
        IrqLockGuard { guard: ManuallyDrop::new(self.inner.lock()), were_enabled }
    }

    /// See `SpinLock::force_unlock`.
    pub unsafe fn force_unlock(&self) {
        self.inner.force_unlock();
    }
}

pub struct IrqLockGuard<'a, T> {
    guard: ManuallyDrop<SpinLockGuard<'a, T>>,
    were_enabled: bool,
}

impl<T> Deref for IrqLockGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.guard
    }
}

impl<T> DerefMut for IrqLockGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.guard
    }
}

impl<T> Drop for IrqLockGuard<'_, T> {
    fn drop(&mut self) {
        // Release the lock, THEN allow interrupts - the reverse order of
        // taking it.
        // SAFETY: dropped exactly once, here, and never used again.
        unsafe { ManuallyDrop::drop(&mut self.guard) };
        arch::restore_interrupts(self.were_enabled);
    }
}
