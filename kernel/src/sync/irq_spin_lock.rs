//! A spinlock that also masks interrupts while held.
//!
//! Data that interrupt handlers touch (the run queue, once the timer
//! preempts in Phase 2.2.4) needs two kinds of protection:
//!
//! - **Against other CPUs:** the atomic spin flag. On today's single CPU it
//!   never actually spins, but the lock is correct once SMP arrives.
//! - **Against this CPU's interrupt handlers:** interrupts are disabled for
//!   the whole critical section. Otherwise an IRQ arriving while the lock is
//!   held would spin forever on a lock its own CPU can never release.
//!
//! # Rules
//!
//! - **Never re-acquire a lock you already hold** (including from code
//!   called under it): on one CPU that is an immediate, silent deadlock.
//! - **NMI handlers must not take an `IrqSpinLock`.** NMIs ignore IF, so
//!   masking interrupts does not keep them out of the critical section.

use core::cell::UnsafeCell;
use core::marker::PhantomData;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, Ordering};

use crate::arch::x86_64::cpu::{
    restore_interrupts, save_and_disable_interrupts, SavedInterruptState,
};

/// A mutual-exclusion lock that disables interrupts while held.
///
/// `const`-constructible, so it can guard a `static`.
pub struct IrqSpinLock<T> {
    locked: AtomicBool,
    value: UnsafeCell<T>,
}

// SAFETY: `IrqSpinLock` hands out access to `value` only through
// `IrqSpinLockGuard`, and at most one guard exists at a time (the `locked`
// flag, acquired with Acquire and released with Release, enforces this).
// Sharing the lock across contexts therefore never yields aliased `&mut T`.
// `T: Send` is required because the guard can move `T`'s contents between
// whichever contexts take the lock.
unsafe impl<T: Send> Sync for IrqSpinLock<T> {}

impl<T> IrqSpinLock<T> {
    /// Create an unlocked lock around `value`.
    pub const fn new(value: T) -> Self {
        Self {
            locked: AtomicBool::new(false),
            value: UnsafeCell::new(value),
        }
    }

    /// Disable interrupts, then acquire the lock, spinning if needed.
    ///
    /// Interrupts stay disabled until the returned guard is dropped, which
    /// restores the interrupt state from *before* this call. Nested locks
    /// therefore compose correctly.
    pub fn lock(&self) -> IrqSpinLockGuard<'_, T> {
        // Disable first: once the flag is held, no interrupt on this CPU may
        // run code that could try to take the same lock.
        let saved = save_and_disable_interrupts();
        while self
            .locked
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            // Wait with plain loads (no cache-line ping-pong) until it looks free.
            while self.locked.load(Ordering::Relaxed) {
                core::hint::spin_loop();
            }
        }
        IrqSpinLockGuard {
            lock: self,
            saved,
            _not_send: PhantomData,
        }
    }

    /// Acquire the lock only if it is free right now; never spins.
    ///
    /// On failure the interrupt state is left exactly as it was.
    pub fn try_lock(&self) -> Option<IrqSpinLockGuard<'_, T>> {
        let saved = save_and_disable_interrupts();
        if self
            .locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            Some(IrqSpinLockGuard {
                lock: self,
                saved,
                _not_send: PhantomData,
            })
        } else {
            restore_interrupts(saved);
            None
        }
    }

    /// `true` if some context currently holds the lock (diagnostics only:
    /// the answer can be stale as soon as it is returned).
    pub fn is_locked(&self) -> bool {
        self.locked.load(Ordering::Relaxed)
    }
}

/// Exclusive access to an [`IrqSpinLock`]'s value; interrupts are disabled
/// for as long as it lives.
#[must_use = "dropping the guard immediately releases the lock"]
pub struct IrqSpinLockGuard<'a, T> {
    lock: &'a IrqSpinLock<T>,
    saved: SavedInterruptState,
    // The saved interrupt state belongs to this CPU, so the guard must not
    // migrate to another context (`*const ()` makes it !Send and !Sync).
    _not_send: PhantomData<*const ()>,
}

impl<T> Deref for IrqSpinLockGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        // SAFETY: the guard's existence proves this context holds the lock,
        // so no other reference to `value` exists.
        unsafe { &*self.lock.value.get() }
    }
}

impl<T> DerefMut for IrqSpinLockGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: as in `deref`; `&mut self` also rules out a concurrent
        // shared borrow through this same guard.
        unsafe { &mut *self.lock.value.get() }
    }
}

impl<T> Drop for IrqSpinLockGuard<'_, T> {
    fn drop(&mut self) {
        // Release before re-enabling interrupts. In the other order, an IRQ
        // landing between the two steps would find the lock still held.
        self.lock.locked.store(false, Ordering::Release);
        restore_interrupts(self.saved);
    }
}
