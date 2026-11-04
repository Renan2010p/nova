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

//! The FPU and SSE units (FreeBSD's `amd64/amd64/fpu.c`).
//!
//! The kernel itself never uses them (it is built soft-float), so only
//! user threads have FPU state, and it changes hands only at a context
//! switch: `cpu_switch` saves the old thread's with `fxsave` and loads the
//! new one's with `fxrstor` (eager switching, as FreeBSD does). A thread
//! starts with the processor's reset state, so no program ever sees
//! another's registers.
//!
//! TODO(xsave): AVX and later state with `xsave`/`xrstor`. Until then
//! `CR4.OSXSAVE` stays clear, so the CPU does not report AVX to programs
//! and libc picks its SSE routines.

use x86::cpufunc::{load_cr0, load_cr4, rcr0, rcr4};
use x86::specialreg::{CR0_EM, CR0_MP, CR0_NE, CR0_TS, CR4_OSFXSR, CR4_OSXMMEXCPT};

/// The `fxsave` area: x87, MMX and SSE state (512 bytes, 16-aligned).
#[repr(C, align(16))]
#[derive(Clone, Copy)]
pub(crate) struct SaveFpu(pub(crate) [u8; 512]);

/// Offset of the x87 control word, and its value after `fninit`: all
/// exceptions masked, 64-bit precision, round to nearest.
const FCW_OFF: usize = 0;
const FCW_INIT: u16 = 0x037f;
/// Offset of `MXCSR`, and its reset value: SSE exceptions masked.
const MXCSR_OFF: usize = 24;
const MXCSR_INIT: u32 = 0x1f80;

impl SaveFpu {
    /// The state of a thread that never ran: the processor's after reset.
    pub(crate) const INIT: SaveFpu = {
        let mut b = [0u8; 512];
        let fcw = FCW_INIT.to_le_bytes();
        b[FCW_OFF] = fcw[0];
        b[FCW_OFF + 1] = fcw[1];
        let mxcsr = MXCSR_INIT.to_le_bytes();
        let mut i = 0;
        while i < 4 {
            b[MXCSR_OFF + i] = mxcsr[i];
            i += 1;
        }
        SaveFpu(b)
    };
}

/// Enables the FPU and SSE on this CPU (`fpuinit`): native error
/// reporting, `wait` checks `TS`, no emulation, `fxsave`/`fxrstor` and
/// SSE exceptions.
pub(crate) fn fpuinit() {
    // SAFETY: only FPU-related bits change; the kernel does not use the
    // FPU, and every amd64 CPU has x87 and SSE2.
    unsafe {
        load_cr0((rcr0() | CR0_MP | CR0_NE) & !(CR0_EM | CR0_TS));
        load_cr4(rcr4() | CR4_OSFXSR | CR4_OSXMMEXCPT);
    }
}
