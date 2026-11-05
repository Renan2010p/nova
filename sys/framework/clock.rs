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

//! The periodic clock interrupt (the mechanism under FreeBSD's
//! `kern_clock.c`). The framework starts the MD timer and calls the
//! service's `hardclock` on every tick; what a tick means (ticks,
//! callouts, scheduling) is the service's business.

use mdi::Clock;

use crate::Md;
use crate::sync::MtxSpin;

/// Name of the event timer driving the clock, as FreeBSD reports it.
pub const ET_NAME: &str = <Md as Clock>::ET_NAME;
/// Quality of that event timer, as FreeBSD reports it.
pub const ET_QUALITY: i32 = <Md as Clock>::ET_QUALITY;

/// The service's tick handler.
static HARDCLOCK: MtxSpin<Option<fn()>> = MtxSpin::new(None);

/// Starts the clock on this CPU at `hz` ticks per second and calls
/// `hardclock` on every tick, in interrupt context: interrupts disabled,
/// on the CPU that took the tick, so it must be short and must not sleep.
///
/// # Panics
///
/// If the clock is already running, or `hz` is 0.
pub fn cpu_initclocks(hz: u32, hardclock: fn()) {
    assert!(hz > 0, "cpu_initclocks: hz must be positive");
    {
        let mut h = HARDCLOCK.lock();
        assert!(h.is_none(), "cpu_initclocks: clock already running");
        *h = Some(hardclock);
    }
    HZ.store(hz, core::sync::atomic::Ordering::Relaxed);
    Md::cpu_initclocks(hz);
}

static HZ: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// Starts this AP's clock at the boot CPU's frequency, with the same
/// handler, if the clock was started (`cpu_initclocks_ap`).
pub(crate) fn cpu_initclocks_ap() {
    let hz = HZ.load(core::sync::atomic::Ordering::Relaxed);
    if hz != 0 {
        Md::cpu_initclocks(hz);
    }
}

/// A clock tick arrived (from `intr::__nova_intr`).
pub(crate) fn clock_intr() {
    // Copy the handler out so it does not run under the lock.
    let handler = *HARDCLOCK.lock();
    if let Some(hardclock) = handler {
        hardclock();
    }
}

/// The machine's real-time clock, in seconds since the Epoch, if any.
pub fn rtc_gettime() -> Option<u64> {
    <crate::Md as mdi::Clock>::rtc_gettime()
}
