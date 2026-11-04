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

//! Blocking and waking threads (the mechanism under FreeBSD's
//! `sleepqueue(9)`).
//!
//! No wakeup is ever lost: the condition is checked again with the queue
//! locked, after the thread is on the queue, and a waker makes the
//! condition true before taking the same lock to wake.

use super::MtxSpin;
use crate::cpu::{assert_no_guard, intr_guard};
use crate::task::{self, TaskQueue};

/// Threads waiting for something. Waiting and waking never allocate: the
/// queue links the threads themselves.
pub struct WaitQueue {
    waiters: MtxSpin<TaskQueue>,
}

impl WaitQueue {
    /// An empty queue.
    pub const fn new() -> Self {
        Self {
            waiters: MtxSpin::new(TaskQueue::new()),
        }
    }

    /// Blocks the current thread until `cond` returns true.
    ///
    /// `cond` runs with a spin lock held and interrupts off: it must be
    /// quick and must not block (typically it reads atomics).
    ///
    /// # Panics
    ///
    /// If it would have to sleep while the caller holds a spin lock or an
    /// interrupt guard, or from the idle thread.
    pub fn wait_until(&self, mut cond: impl FnMut() -> bool) {
        loop {
            if cond() {
                return;
            }
            assert_no_guard("sleeping thread holds a spin lock or critical section");
            let guard = intr_guard();
            {
                let mut waiters = self.waiters.lock();
                if cond() {
                    return;
                }
                let current = task::current();
                current.mark_blocked();
                waiters.push_back(current);
            }
            task::reschedule(&guard);
        }
    }

    /// Wakes the thread that has waited longest. Returns false if none was
    /// waiting.
    pub fn wake_one(&self) -> bool {
        let next = self.waiters.lock().pop_front();
        match next {
            Some(t) => {
                task::unpark(t);
                true
            }
            None => false,
        }
    }

    /// Wakes every thread waiting now (a thread that blocks again while
    /// this runs waits for the next wakeup).
    pub fn wake_all(&self) {
        let mut waiters = core::mem::take(&mut *self.waiters.lock());
        while let Some(t) = waiters.pop_front() {
            task::unpark(t);
        }
    }
}

impl Default for WaitQueue {
    fn default() -> Self {
        Self::new()
    }
}
