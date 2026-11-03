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

//! The local APIC (FreeBSD's `x86/x86/local_apic.c`), in x2APIC mode.
//!
//! In x2APIC mode every APIC register is an MSR, so no MMIO mapping is
//! needed. Each CPU has its own local APIC; everything here acts on the
//! APIC of the calling CPU.
//!
//! TODO(xapic): CPUs (or hypervisors) without x2APIC need the MMIO xAPIC,
//! mapped uncached; until then the kernel refuses to boot on them.

use crate::cpufunc::{cpuid, ia32_pause, rdmsr, wrmsr};
use crate::i8254::{self, TIMER_FREQ};
use crate::identcpu::has_x2apic;
use crate::specialreg::{APICBASE_ENABLED, APICBASE_X2APIC, MSR_APICBASE};

const MSR_APIC_ID: u32 = 0x802;
const MSR_APIC_TPR: u32 = 0x808;
const MSR_APIC_EOI: u32 = 0x80b;
const MSR_APIC_SVR: u32 = 0x80f;
const MSR_APIC_LVT_TIMER: u32 = 0x832;
const MSR_APIC_ICR_TIMER: u32 = 0x838;
const MSR_APIC_CCR_TIMER: u32 = 0x839;
const MSR_APIC_DCR_TIMER: u32 = 0x83e;

const APIC_SVR_ENABLE: u64 = 1 << 8;
const APIC_LVT_M: u64 = 1 << 16;
const APIC_LVTT_TM_PERIODIC: u64 = 1 << 17;
/// Timer divide configuration: divide by 16.
const APIC_TDCR_16: u64 = 0x3;

/// Length of the calibration window: 1/100 s.
const CALIBRATE_DIV: u32 = 100;
/// Polls of the i8254 before giving up on it.
const CALIBRATE_SPIN_LIMIT: u32 = 1 << 26;

/// Why the local APIC cannot be used.
#[derive(Debug)]
pub enum LapicError {
    /// The CPU does not support x2APIC mode.
    NoX2apic,
    /// The i8254 never finished counting, so the timer is uncalibrated.
    NoCalibration,
}

/// Enables this CPU's local APIC in x2APIC mode, with `spurious_vector`
/// for spurious interrupts, all priorities accepted, and the timer masked
/// (`lapic_init` / `lapic_setup`).
pub fn lapic_init(spurious_vector: u8) -> Result<(), LapicError> {
    if !has_x2apic() {
        return Err(LapicError::NoX2apic);
    }
    // SAFETY: MSR_APICBASE and the x2APIC MSRs exist (checked above). The
    // APIC must be enabled in xAPIC mode before switching to x2APIC.
    unsafe {
        let base = rdmsr(MSR_APICBASE);
        wrmsr(MSR_APICBASE, base | APICBASE_ENABLED);
        wrmsr(MSR_APICBASE, base | APICBASE_ENABLED | APICBASE_X2APIC);
        wrmsr(MSR_APIC_TPR, 0);
        wrmsr(MSR_APIC_LVT_TIMER, APIC_LVT_M);
        wrmsr(MSR_APIC_SVR, APIC_SVR_ENABLE | u64::from(spurious_vector));
    }
    Ok(())
}

/// This CPU's APIC id.
pub fn lapic_id() -> u32 {
    if !has_x2apic() {
        return cpuid(1, 0)[1] >> 24;
    }
    // SAFETY: x2APIC MSRs exist; reading the ID has no side effects. If
    // the APIC is still in xAPIC mode this would #GP, but `lapic_init`
    // runs before anything asks.
    unsafe { rdmsr(MSR_APIC_ID) as u32 }
}

/// Signals end of interrupt to this CPU's local APIC.
pub fn lapic_eoi() {
    // SAFETY: writing 0 to the EOI MSR acknowledges the interrupt being
    // serviced; `lapic_init` put the APIC in x2APIC mode.
    unsafe { wrmsr(MSR_APIC_EOI, 0) }
}

/// Measures the timer's input frequency (after the divide-by-16), in
/// ticks per second, against the i8254.
pub fn lapic_calibrate_timer() -> Result<u32, LapicError> {
    let count = (TIMER_FREQ / CALIBRATE_DIV) as u16;
    let elapsed;
    // SAFETY: x2APIC timer MSRs; the timer is masked, so counting down
    // raises no interrupt.
    unsafe {
        wrmsr(MSR_APIC_DCR_TIMER, APIC_TDCR_16);
        wrmsr(MSR_APIC_LVT_TIMER, APIC_LVT_M);
        i8254::ch2_start(count);
        wrmsr(MSR_APIC_ICR_TIMER, u64::from(u32::MAX));
        let mut spins = 0;
        while !i8254::ch2_expired() {
            spins += 1;
            if spins == CALIBRATE_SPIN_LIMIT {
                return Err(LapicError::NoCalibration);
            }
            ia32_pause();
        }
        elapsed = u32::MAX - rdmsr(MSR_APIC_CCR_TIMER) as u32;
        wrmsr(MSR_APIC_ICR_TIMER, 0);
    }
    let freq = u64::from(elapsed) * u64::from(TIMER_FREQ) / u64::from(count);
    u32::try_from(freq).map_err(|_| LapicError::NoCalibration)
}

/// Starts the periodic timer: `vector` every `period` timer ticks.
pub fn lapic_timer_periodic(vector: u8, period: u32) {
    // SAFETY: x2APIC timer MSRs. Once unmasked the timer raises `vector`,
    // which the caller has installed in the IDT.
    unsafe {
        wrmsr(MSR_APIC_DCR_TIMER, APIC_TDCR_16);
        wrmsr(
            MSR_APIC_LVT_TIMER,
            APIC_LVTT_TM_PERIODIC | u64::from(vector),
        );
        wrmsr(MSR_APIC_ICR_TIMER, u64::from(period.max(1)));
    }
}
