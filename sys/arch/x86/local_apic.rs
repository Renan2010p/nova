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

use libkern::conv::trunc_u32;
use core::sync::atomic::{AtomicU32, Ordering};

use crate::cpufunc::{cpuid, ia32_pause, rdmsr, wrmsr};
use crate::i8254::{self, TIMER_FREQ};
use crate::identcpu::has_x2apic;
use crate::specialreg::{APICBASE_ENABLED, APICBASE_X2APIC, MSR_APICBASE};
use crate::tsc::delay;

const MSR_APIC_ID: u32 = 0x802;
const MSR_APIC_TPR: u32 = 0x808;
const MSR_APIC_EOI: u32 = 0x80b;
const MSR_APIC_ICR: u32 = 0x830;
const MSR_APIC_SVR: u32 = 0x80f;
const MSR_APIC_LVT_TIMER: u32 = 0x832;
const MSR_APIC_LVT_LINT0: u32 = 0x835;
const MSR_APIC_LVT_ERROR: u32 = 0x837;
const MSR_APIC_ICR_TIMER: u32 = 0x838;
const MSR_APIC_CCR_TIMER: u32 = 0x839;
const MSR_APIC_DCR_TIMER: u32 = 0x83e;

const APIC_SVR_ENABLE: u64 = 1 << 8;
const APIC_LVT_M: u64 = 1 << 16;
/// Delivery mode ExtINT: the 8259's interrupts, as in "virtual wire" mode.
const APIC_LVT_DM_EXTINT: u64 = 0x700;
const APIC_LVTT_TM_PERIODIC: u64 = 1 << 17;
/// Timer divide configuration: divide by 16.
const APIC_TDCR_16: u64 = 0x3;

// Interrupt command register (`APIC_DELMODE_*`, `APIC_LEVEL_*`).
const APIC_DELMODE_FIXED: u32 = 0x000;
const APIC_DELMODE_INIT: u32 = 0x500;
const APIC_DELMODE_STARTUP: u32 = 0x600;
const APIC_LEVEL_ASSERT: u32 = 0x4000;
const APIC_TRIGMOD_LEVEL: u32 = 0x8000;

/// Measured timer frequency, shared by all CPUs (same clock source).
static LAPIC_TIMER_FREQ: AtomicU32 = AtomicU32::new(0);

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
        // LINT0 carries the 8259s in virtual wire mode: masked, as in
        // FreeBSD with APICs (ExtINT, so unmasking it would restore the
        // legacy path). LINT1 stays as the firmware set it (NMI).
        wrmsr(MSR_APIC_LVT_LINT0, APIC_LVT_M | APIC_LVT_DM_EXTINT);
        wrmsr(MSR_APIC_LVT_ERROR, APIC_LVT_M);
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
    unsafe { trunc_u32(rdmsr(MSR_APIC_ID)) } // x2APIC ids are 32 bits
}

/// Signals end of interrupt to this CPU's local APIC.
pub fn lapic_eoi() {
    // SAFETY: writing 0 to the EOI MSR acknowledges the interrupt being
    // serviced; `lapic_init` put the APIC in x2APIC mode.
    unsafe { wrmsr(MSR_APIC_EOI, 0) }
}

/// The timer's input frequency (after the divide-by-16), in ticks per
/// second: measured against the i8254 the first time, then remembered, so
/// the APs never touch the i8254 (FreeBSD calibrates once, too).
pub fn lapic_calibrate_timer() -> Result<u32, LapicError> {
    match LAPIC_TIMER_FREQ.load(Ordering::Relaxed) {
        0 => {
            let freq = lapic_measure_timer()?;
            LAPIC_TIMER_FREQ.store(freq, Ordering::Relaxed);
            Ok(freq)
        }
        freq => Ok(freq),
    }
}

fn lapic_measure_timer() -> Result<u32, LapicError> {
    let count = u16::try_from(TIMER_FREQ / CALIBRATE_DIV).expect("a calibration period fits the 16-bit counter");
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
        elapsed = u32::MAX - trunc_u32(rdmsr(MSR_APIC_CCR_TIMER)); // a 32-bit register
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

/// Writes the interrupt command register (`lapic_ipi_raw`). In x2APIC mode
/// the write is the whole send: there is no delivery status to wait for.
fn lapic_ipi_raw(icrlo: u32, apic_id: u32) {
    // SAFETY: x2APIC mode (set by `lapic_init` on this CPU); sending an IPI
    // touches no memory of this CPU.
    unsafe { wrmsr(MSR_APIC_ICR, (u64::from(apic_id) << 32) | u64::from(icrlo)) }
}

/// Sends interrupt `vector` to the CPU with APIC id `apic_id`
/// (`lapic_ipi_vectored`).
pub fn lapic_ipi_vectored(vector: u8, apic_id: u32) {
    lapic_ipi_raw(
        APIC_DELMODE_FIXED | APIC_LEVEL_ASSERT | u32::from(vector),
        apic_id,
    );
}

/// Wakes an application processor with the INIT-SIPI-SIPI sequence, with
/// the delays of FreeBSD's `ipi_startup` (Intel MP spec, section B.4). It
/// starts executing in real mode at physical address `vector << 12`.
///
/// x2APIC has no "INIT level de-assert", so unlike xAPIC FreeBSD there is
/// no second INIT.
pub fn ipi_startup(apic_id: u32, vector: u8) {
    lapic_ipi_raw(
        APIC_TRIGMOD_LEVEL | APIC_LEVEL_ASSERT | APIC_DELMODE_INIT,
        apic_id,
    );
    delay(10_000); // ~10 ms
    for _ in 0..2 {
        lapic_ipi_raw(
            APIC_LEVEL_ASSERT | APIC_DELMODE_STARTUP | u32::from(vector),
            apic_id,
        );
        delay(200); // ~200 us
    }
}
