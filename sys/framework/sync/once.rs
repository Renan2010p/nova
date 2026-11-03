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

//! A value written once, then read by anyone without a lock, even from a
//! panic or an interrupt handler.

use core::cell::UnsafeCell;
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicU8, Ordering};

const EMPTY: u8 = 0;
const WRITING: u8 = 1;
const READY: u8 = 2;

/// A cell that can be set once.
pub struct OnceLock<T> {
    state: AtomicU8,
    value: UnsafeCell<MaybeUninit<T>>,
}

// SAFETY: the value is written once, by the thread that moved `state` from
// EMPTY to WRITING, and only read after `state` is READY (Release/Acquire),
// never mutated afterwards. Sharing it needs `T: Sync`; setting it from
// another thread moves a `T` there, which needs `T: Send`.
unsafe impl<T: Send + Sync> Sync for OnceLock<T> {}

impl<T> OnceLock<T> {
    /// An empty cell.
    pub const fn new() -> Self {
        Self {
            state: AtomicU8::new(EMPTY),
            value: UnsafeCell::new(MaybeUninit::uninit()),
        }
    }

    /// Stores `value` if the cell is empty; otherwise gives it back.
    pub fn set(&self, value: T) -> Result<(), T> {
        if self
            .state
            .compare_exchange(EMPTY, WRITING, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            return Err(value);
        }
        // SAFETY: winning the EMPTY -> WRITING race makes this the only
        // writer, and no reader looks before READY.
        unsafe { (*self.value.get()).write(value) };
        self.state.store(READY, Ordering::Release);
        Ok(())
    }

    /// The value, if it has been set.
    pub fn get(&self) -> Option<&T> {
        if self.state.load(Ordering::Acquire) != READY {
            return None;
        }
        // SAFETY: READY means the value was fully written (Acquire pairs
        // with the Release in `set`) and will never change again.
        Some(unsafe { (*self.value.get()).assume_init_ref() })
    }
}

impl<T> Default for OnceLock<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Drop for OnceLock<T> {
    fn drop(&mut self) {
        if *self.state.get_mut() == READY {
            // SAFETY: READY means initialized; `&mut self` means no readers.
            unsafe { self.value.get_mut().assume_init_drop() };
        }
    }
}
