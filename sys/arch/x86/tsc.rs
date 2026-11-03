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

//! The time-stamp counter (FreeBSD's `x86/x86/tsc.c`): its frequency,
//! measured against the i8254 at boot, and `DELAY` on top of it.

use core::sync::atomic::{AtomicU64, Ordering};

use crate::cpufunc::{ia32_pause, rdtsc};
use crate::i8254::{self, TIMER_FREQ};

/// TSC ticks per second (FreeBSD's `tsc_freq`); 0 until calibrated.
static TSC_FREQ: AtomicU64 = AtomicU64::new(0);

/// Length of the calibration window: 1/100 s.
const CALIBRATE_DIV: u32 = 100;
/// Polls of the i8254 before giving up on it.
const CALIBRATE_SPIN_LIMIT: u32 = 1 << 26;

/// Measures the TSC frequency (`init_TSC`). Returns it, or 0 if the
/// i8254 did not count.
pub fn tsc_calibrate() -> u64 {
    let count = (TIMER_FREQ / CALIBRATE_DIV) as u16;
    i8254::ch2_start(count);
    let start = rdtsc();
    let mut spins = 0;
    while !i8254::ch2_expired() {
        spins += 1;
        if spins == CALIBRATE_SPIN_LIMIT {
            return 0;
        }
        ia32_pause();
    }
    let freq = (rdtsc() - start) * u64::from(TIMER_FREQ) / u64::from(count);
    TSC_FREQ.store(freq, Ordering::Relaxed);
    freq
}

/// The measured TSC frequency, 0 if unknown.
pub fn tsc_freq() -> u64 {
    TSC_FREQ.load(Ordering::Relaxed)
}

/// Busy-waits at least `us` microseconds (`DELAY`). Falls back to the
/// i8254 if the TSC was not calibrated.
pub fn delay(us: u32) {
    let freq = tsc_freq();
    if freq == 0 {
        // At most 1/20 s per i8254 countdown.
        let mut left = u64::from(us);
        while left > 0 {
            let chunk = left.min(50_000);
            i8254::ch2_start((chunk * u64::from(TIMER_FREQ) / 1_000_000) as u16);
            while !i8254::ch2_expired() {
                ia32_pause();
            }
            left -= chunk;
        }
        return;
    }
    let end = rdtsc() + freq * u64::from(us) / 1_000_000;
    while rdtsc() < end {
        ia32_pause();
    }
}
