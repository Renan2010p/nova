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
