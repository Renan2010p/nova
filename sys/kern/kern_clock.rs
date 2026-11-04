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

//! The system clock (FreeBSD's `kern/kern_clock.c`): `hardclock` runs
//! `hz` times a second, counts `ticks`, gives the scheduler its tick and
//! wakes timed sleepers.

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use crate::{kern_synch, printf, sched_4bsd};

static TICKS: AtomicU64 = AtomicU64::new(0);
static HZ: AtomicU32 = AtomicU32::new(0);

/// Starts the clock at `hz` ticks per second (`initclocks`), and reports
/// it the way FreeBSD's event timer and timecounter code do.
pub fn initclocks(hz: u32) {
    HZ.store(hz, Ordering::Relaxed);
    printf!(
        "Event timer \"{}\" quality {}\n",
        framework::clock::ET_NAME,
        framework::clock::ET_QUALITY
    );
    framework::clock::cpu_initclocks(hz, hardclock);
}

/// Reports the tick length (`inittimecounter`).
pub fn inittimecounter() {
    let usec = 1_000_000 / hz();
    printf!(
        "Timecounters tick every {}.{:03} msec\n",
        usec / 1000,
        usec % 1000
    );
}

/// Called by the framework on every clock tick, in interrupt context.
/// Every CPU's clock calls it; only the boot CPU advances `ticks` and
/// wakes timed sleepers, as one CPU does in FreeBSD's `hardclock`.
fn hardclock() {
    if framework::cpu::curcpu() == framework::cpu::CpuId::BSP {
        TICKS.fetch_add(1, Ordering::Relaxed);
        kern_synch::tick_wakeup();
        crate::tty::tty_poll();
    }
    sched_4bsd::sched_clock();
}

/// Clock ticks since `initclocks` (FreeBSD's `ticks`).
pub fn ticks() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

/// Clock frequency (`hz`), 0 before `initclocks`.
pub fn hz() -> u32 {
    HZ.load(Ordering::Relaxed)
}

/// Seconds since the clock started (`time_uptime`).
pub fn uptime_secs() -> u64 {
    match hz() {
        0 => 0,
        hz => ticks() / u64::from(hz),
    }
}

/// A clock a program can read (`clockid_t`, by name).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClockId {
    /// Wall-clock time (`CLOCK_REALTIME`).
    Realtime,
    /// Time since an arbitrary point, never set back (`CLOCK_MONOTONIC`,
    /// `CLOCK_UPTIME`: no suspend yet, so they are the same).
    Monotonic,
    /// Whole seconds of wall-clock time (`CLOCK_SECOND`).
    Second,
}

/// A point in time (`struct timespec`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timespec {
    /// Seconds.
    pub sec: i64,
    /// Nanoseconds, `0..1_000_000_000`.
    pub nsec: i64,
}

/// Reads `clock` (`kern_clock_gettime`).
///
/// TODO(timecounter): a precise timecounter (TSC) instead of ticks, and
/// wall-clock time from the RTC: until then the clocks count from boot,
/// in steps of one tick, and the wall clock starts at the Epoch.
pub fn kern_clock_gettime(clock: ClockId) -> Timespec {
    let hz = u64::from(hz().max(1));
    let t = ticks();
    let sec = i64::try_from(t / hz).unwrap_or(i64::MAX);
    let nsec = i64::try_from((t % hz) * (1_000_000_000 / hz)).unwrap_or(0);
    match clock {
        ClockId::Realtime | ClockId::Monotonic => Timespec { sec, nsec },
        ClockId::Second => Timespec { sec, nsec: 0 },
    }
}
