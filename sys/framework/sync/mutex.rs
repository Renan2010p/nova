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

//! Sleep mutexes (FreeBSD's default `mtx`, `MTX_DEF`): a thread that finds
//! the lock taken blocks until it is released, instead of spinning.

use core::cell::UnsafeCell;
use core::marker::PhantomData;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, Ordering};

use super::WaitQueue;
use crate::cpu::assert_no_guard;

/// A mutual exclusion lock whose waiters sleep.
pub struct Mutex<T: ?Sized> {
    locked: AtomicBool,
    waiters: WaitQueue,
    data: UnsafeCell<T>,
}

// SAFETY: `data` is reached only through a `MutexGuard`, and `locked`
// (Acquire on lock, Release on unlock) lets at most one exist at a time,
// so the lock only moves `T` between threads: `T: Send` is enough.
unsafe impl<T: ?Sized + Send> Sync for Mutex<T> {}
// SAFETY: the mutex owns its `T`.
unsafe impl<T: ?Sized + Send> Send for Mutex<T> {}

impl<T> Mutex<T> {
    /// An unlocked mutex.
    pub const fn new(data: T) -> Self {
        Self {
            locked: AtomicBool::new(false),
            waiters: WaitQueue::new(),
            data: UnsafeCell::new(data),
        }
    }
}

impl<T: ?Sized> Mutex<T> {
    /// Acquires the mutex, sleeping while another thread holds it
    /// (`mtx_lock`).
    ///
    /// # Panics
    ///
    /// If the caller holds a spin lock or interrupt guard: it could have
    /// to sleep (FreeBSD's WITNESS reports the same mistake).
    pub fn lock(&self) -> MutexGuard<'_, T> {
        assert_no_guard("acquiring blockable sleep lock with spinlock or critical section held");
        loop {
            if let Some(g) = self.try_lock() {
                return g;
            }
            self.waiters
                .wait_until(|| !self.locked.load(Ordering::Relaxed));
        }
    }

    /// Acquires the mutex only if it is free (`mtx_trylock`).
    pub fn try_lock(&self) -> Option<MutexGuard<'_, T>> {
        self.locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .ok()
            .map(|_| MutexGuard {
                mutex: self,
                _not_send: PhantomData,
            })
    }
}

/// Proof that a [`Mutex`] is held; releases it (`mtx_unlock`) on drop.
pub struct MutexGuard<'a, T: ?Sized> {
    mutex: &'a Mutex<T>,
    _not_send: PhantomData<*const ()>,
}

impl<T: ?Sized> Deref for MutexGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        // SAFETY: the guard exists, so this thread holds the lock.
        unsafe { &*self.mutex.data.get() }
    }
}

impl<T: ?Sized> DerefMut for MutexGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: as in `deref`; `&mut self` makes this access exclusive.
        unsafe { &mut *self.mutex.data.get() }
    }
}

impl<T: ?Sized> Drop for MutexGuard<'_, T> {
    fn drop(&mut self) {
        self.mutex.locked.store(false, Ordering::Release);
        self.mutex.waiters.wake_one();
    }
}
