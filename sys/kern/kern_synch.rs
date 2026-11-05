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

//! Sleeping for a while (FreeBSD's `kern/kern_synch.c`: `pause(9)`).
//!
//! For now every timed sleeper waits on one queue that `hardclock` wakes
//! on each tick (the old BSD `lbolt`), and checks its own deadline.
//!
//! A sleep a signal may interrupt ([`sleep_sig`], FreeBSD's `PCATCH`)
//! records its wait channel in the process, so that sending a signal
//! wakes it.
//!
//! TODO(callout): a callout wheel (`kern_timeout.c`), so that sleepers
//! are woken only when their time is up.

use framework::mm::KArc;
use framework::sync::WaitQueue;
use sys::errno::{Errno, KResult};

use crate::kern_clock;
use crate::kern_proc::Proc;

/// Woken on every clock tick.
static TICK_WAIT: WaitQueue = WaitQueue::new();

/// Sleeps for `timo` clock ticks, at least one (`pause`). `wmesg` names
/// the wait, as it will appear in `ps`/`top`.
pub fn pause(wmesg: &'static str, timo: u32) {
    let _ = wmesg;
    let deadline = kern_clock::ticks() + u64::from(timo.max(1));
    TICK_WAIT.wait_until(|| kern_clock::ticks() >= deadline);
}

/// Called from `hardclock` on every tick.
pub(crate) fn tick_wakeup() {
    TICK_WAIT.wake_all();
}


/// What a process sleeps on (FreeBSD's wait channel, `wchan`): a queue a
/// signal sent to the process wakes, so that its sleep ends.
#[derive(Clone)]
pub enum WChan {
    /// A queue that lives forever.
    Static(&'static WaitQueue),
    /// A queue shared with an object (a pipe).
    Shared(KArc<WaitQueue>),
}

impl WChan {
    fn queue(&self) -> &WaitQueue {
        match self {
            WChan::Static(q) => q,
            WChan::Shared(q) => q,
        }
    }

    /// Wakes everyone sleeping on it; each checks what it waits for again.
    pub fn wake(&self) {
        self.queue().wake_all();
    }
}

/// Sleeps on `chan` until `cond` holds, or a signal comes (`tsleep` with
/// `PCATCH`): then `EINTR`, or `ERESTART` if the call should be
/// restarted (see [`crate::kern_sig::sig_interrupt`]). `cond` must be
/// quick and must not sleep (it runs with interrupts off).
pub fn sleep_sig(p: &Proc, chan: WChan, mut cond: impl FnMut() -> bool) -> KResult<()> {
    *p.wchan.lock() = Some(chan.clone());
    chan.queue().wait_until(|| cond() || p.sig_pending());
    *p.wchan.lock() = None;
    if cond() {
        return Ok(());
    }
    Err(crate::kern_sig::sig_interrupt(p))
}

/// Sleeps at least `ns` nanoseconds, or until a signal comes (`EINTR`:
/// `nanosleep` is never restarted).
pub fn kern_nanosleep_sig(p: &Proc, ns: u64) -> KResult<()> {
    let hz = u64::from(kern_clock::hz());
    let ticks = u128::from(ns) * u128::from(hz);
    let ticks = u64::try_from(ticks.div_ceil(1_000_000_000) + 1).unwrap_or(u64::MAX);
    let deadline = kern_clock::ticks().saturating_add(ticks);
    match sleep_sig(p, WChan::Static(&TICK_WAIT), || kern_clock::ticks() >= deadline) {
        Err(Errno::ERESTART) => Err(Errno::EINTR),
        r => r,
    }
}
