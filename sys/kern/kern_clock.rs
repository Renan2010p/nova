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
//! `hz` times a second and counts `ticks`. Callouts, statistics and
//! scheduler ticks will hang off it.

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

static TICKS: AtomicU64 = AtomicU64::new(0);
static HZ: AtomicU32 = AtomicU32::new(0);

/// Starts the clock at `hz` ticks per second (`initclocks`).
pub fn initclocks(hz: u32) {
    HZ.store(hz, Ordering::Relaxed);
    framework::clock::cpu_initclocks(hz, hardclock);
}

/// Called by the framework on every clock tick, in interrupt context.
fn hardclock() {
    // TODO(smp): with a clock on every CPU, only one of them advances
    // `ticks`, as FreeBSD's hardclock does.
    TICKS.fetch_add(1, Ordering::Relaxed);
}

/// Clock ticks since `initclocks` (FreeBSD's `ticks`).
pub fn ticks() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

/// Clock frequency (`hz`), 0 before `initclocks`.
pub fn hz() -> u32 {
    HZ.load(Ordering::Relaxed)
}
