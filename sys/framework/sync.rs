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

//! Locks usable from any context (FreeBSD's spin mutexes, `MTX_SPIN`).
//!
//! Sleep mutexes, sx locks and rwlocks need the scheduler; they will be
//! built in `kern` on top of the primitives here.

use core::ops::{Deref, DerefMut};

use libkern::{SpinLock, SpinLockGuard};

use crate::cpu::{IntrGuard, PinnedToCpu, intr_guard};
use crate::sealed;

/// A spin mutex that disables interrupts on this CPU while held, so it is
/// safe to take from interrupt handlers and on any number of CPUs.
pub struct MtxSpin<T: ?Sized> {
    lock: SpinLock<T>,
}

impl<T> MtxSpin<T> {
    /// Creates an unlocked mutex.
    pub const fn new(data: T) -> Self {
        Self {
            lock: SpinLock::new(data),
        }
    }
}

impl<T: ?Sized> MtxSpin<T> {
    /// Disables interrupts, then spins until the mutex is acquired
    /// (`mtx_lock_spin`).
    pub fn lock(&self) -> MtxSpinGuard<'_, T> {
        let intr = intr_guard();
        MtxSpinGuard {
            inner: self.lock.lock(),
            _intr: intr,
        }
    }
}

/// Proof that an [`MtxSpin`] is held (`mtx_unlock_spin` on drop). Also
/// pins the holder to its CPU.
pub struct MtxSpinGuard<'a, T: ?Sized> {
    // Field order matters: fields drop in declaration order, so the lock
    // is released before interrupts are re-enabled.
    inner: SpinLockGuard<'a, T>,
    _intr: IntrGuard,
}

impl<T: ?Sized> Deref for MtxSpinGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.inner
    }
}

impl<T: ?Sized> DerefMut for MtxSpinGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.inner
    }
}

impl<T: ?Sized> sealed::Sealed for MtxSpinGuard<'_, T> {}
impl<T: ?Sized> PinnedToCpu for MtxSpinGuard<'_, T> {}
