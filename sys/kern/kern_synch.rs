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
//! TODO(callout): a callout wheel (`kern_timeout.c`), so that sleepers
//! are woken only when their time is up, and `tsleep`/`wakeup` channels.

use framework::sync::WaitQueue;

use crate::kern_clock;

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
