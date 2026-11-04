/*-
 * SPDX-License-Identifier: MPL-2.0
 *
 * Copyright (c) 2026 Renan Lucas Vieira Hilário
 *
 * This file is part of the Nova Project, the kernel of the
 * Renux "Nova" Project.
 *
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

//! A plain test-and-test-and-set spin lock.
//!
//! This lock does **not** touch interrupts: `libkern` sits below the
//! machine layer and cannot. Kernel code that may race with an interrupt
//! handler must use `kern::kern_mutex::MtxSpin`, which wraps this lock and
//! disables interrupts while it is held (FreeBSD's `mtx_lock_spin`).

use core::cell::UnsafeCell;
use core::hint::spin_loop;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, Ordering};

/// Mutual exclusion by busy-waiting.
pub struct SpinLock<T: ?Sized> {
    locked: AtomicBool,
    data: UnsafeCell<T>,
}

// SAFETY: `data` is only reachable through a guard, and at most one guard
// exists at a time (`locked` is acquired with Acquire and released with
// Release). Sharing the lock across CPUs therefore only moves `T` between
// them, which is sound when `T: Send`.
unsafe impl<T: ?Sized + Send> Sync for SpinLock<T> {}
// SAFETY: the lock owns its `T`; sending the lock sends the `T`.
unsafe impl<T: ?Sized + Send> Send for SpinLock<T> {}

impl<T> SpinLock<T> {
    /// Creates an unlocked lock around `data`.
    pub const fn new(data: T) -> Self {
        Self {
            locked: AtomicBool::new(false),
            data: UnsafeCell::new(data),
        }
    }
}

impl<T: ?Sized> SpinLock<T> {
    /// Spins until the lock is acquired.
    pub fn lock(&self) -> SpinLockGuard<'_, T> {
        while self
            .locked
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            // Wait on a plain load so the cache line stays shared.
            while self.locked.load(Ordering::Relaxed) {
                spin_loop();
            }
        }
        SpinLockGuard { lock: self }
    }

    /// Acquires the lock only if it is free right now.
    pub fn try_lock(&self) -> Option<SpinLockGuard<'_, T>> {
        self.locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .ok()
            .map(|_| SpinLockGuard { lock: self })
    }
}

/// Proof that the lock is held; releases it when dropped.
pub struct SpinLockGuard<'a, T: ?Sized> {
    lock: &'a SpinLock<T>,
}

impl<T: ?Sized> Deref for SpinLockGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        // SAFETY: the guard exists, so this CPU holds the lock and no other
        // reference to `data` can be live.
        unsafe { &*self.lock.data.get() }
    }
}

impl<T: ?Sized> DerefMut for SpinLockGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: as in `deref`; `&mut self` makes this access exclusive.
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T: ?Sized> Drop for SpinLockGuard<'_, T> {
    fn drop(&mut self) {
        self.lock.locked.store(false, Ordering::Release);
    }
}

#[cfg(test)]
#[allow(clippy::as_conversions, reason = "host test fixtures, not kernel code")]
mod tests {
    use super::SpinLock;

    #[test]
    fn lock_unlock() {
        let l = SpinLock::new(1);
        *l.lock() += 1;
        assert_eq!(*l.lock(), 2);
    }

    #[test]
    fn try_lock_fails_while_held() {
        let l = SpinLock::new(());
        let g = l.lock();
        assert!(l.try_lock().is_none());
        drop(g);
        assert!(l.try_lock().is_some());
    }

    #[test]
    fn contended() {
        let l = SpinLock::new(0u64);
        std::thread::scope(|s| {
            for _ in 0..4 {
                s.spawn(|| {
                    for _ in 0..10_000 {
                        *l.lock() += 1;
                    }
                });
            }
        });
        assert_eq!(*l.lock(), 40_000);
    }
}
