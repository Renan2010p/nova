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

//! The i8254 programmable interval timer (FreeBSD's `x86/isa/clock.c`),
//! used only as a known-frequency reference to calibrate other timers.
//!
//! Channel 2 is used, because its output can be polled through port 0x61
//! without taking an interrupt; channel 0 (the legacy IRQ 0 timer) is left
//! alone.

use crate::cpufunc::{inb, outb};

/// Input frequency of the i8254, in Hz (FreeBSD's `TIMER_FREQ`).
pub const TIMER_FREQ: u32 = 1_193_182;

const TIMER_CNTR2: u16 = 0x42;
const TIMER_MODE: u16 = 0x43;
/// Channel 2, low then high byte, mode 0 (interrupt on terminal count).
const TIMER_SEL2_ONESHOT: u8 = 0xb0;
/// Port 0x61: NMI status and control.
const NMISC: u16 = 0x61;
const NMISC_GATE2: u8 = 0x01;
const NMISC_SPKR: u8 = 0x02;
const NMISC_OUT2: u8 = 0x20;

/// Starts a one-shot countdown of `count` input clocks on channel 2.
/// Poll [`ch2_expired`] for the end.
pub fn ch2_start(count: u16) {
    // SAFETY: ports 0x42, 0x43 and 0x61 drive the i8254 channel 2 and the
    // speaker gate on every PC; nothing else in the kernel uses them, and
    // the speaker stays off.
    unsafe {
        let v = inb(NMISC);
        outb(NMISC, (v & !NMISC_SPKR) | NMISC_GATE2);
        outb(TIMER_MODE, TIMER_SEL2_ONESHOT);
        outb(TIMER_CNTR2, count as u8);
        outb(TIMER_CNTR2, (count >> 8) as u8); // counting starts here
    }
}

/// True once the countdown started by [`ch2_start`] has reached zero.
pub fn ch2_expired() -> bool {
    // SAFETY: reading port 0x61 has no side effects.
    unsafe { inb(NMISC) & NMISC_OUT2 != 0 }
}
