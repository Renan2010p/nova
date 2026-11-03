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

//! The MD per-CPU area (FreeBSD's `amd64/include/pcpu.h`).
//!
//! Each CPU points `MSR_GSBASE` at its own [`Pcpu`], so one `%gs`-relative
//! load answers "which CPU am I?" without locks or table lookups. This is
//! the root of all SMP support: the framework's `curcpu()` and
//! `CpuLocal<T>` are built on it.
//!
//! The area will grow fields FreeBSD keeps here too (`curthread`, the TSS,
//! the kernel stack for syscalls); they are read in assembly at fixed
//! offsets, hence `#[repr(C)]`.

use core::arch::asm;
use core::mem::offset_of;

use x86::cpufunc::wrmsr;
use x86::specialreg::MSR_GSBASE;

use crate::param::MAXCPU;

/// MD per-CPU data, at `%gs:0` on its CPU.
#[repr(C)]
pub struct Pcpu {
    /// This CPU's id (`pc_cpuid`).
    cpuid: u32,
}

/// One area per possible CPU, with `cpuid` filled in at build time.
static PCPU: [Pcpu; MAXCPU] = {
    let mut a = [const { Pcpu { cpuid: 0 } }; MAXCPU];
    let mut i = 0;
    while i < MAXCPU {
        a[i].cpuid = i as u32;
        i += 1;
    }
    a
};

/// Makes `%gs` point at CPU `cpuid`'s area. Called by each CPU on itself,
/// first thing: `hammer_time` for the boot CPU, `init_secondary` for APs.
pub(crate) fn pcpu_init(cpuid: u32) {
    let pcpu = &PCPU[cpuid as usize];
    // SAFETY: MSR_GSBASE exists on every amd64 CPU. The new base points at
    // a static that lives forever and whose `cpuid` is this CPU's, which
    // is exactly the invariant `curcpu()` relies on.
    unsafe { wrmsr(MSR_GSBASE, pcpu as *const Pcpu as u64) }
}

/// Id of the executing CPU.
#[inline(always)]
pub(crate) fn curcpu() -> u32 {
    let id: u32;
    // SAFETY: `pcpu_init` ran on this CPU before any MI code, so `%gs:0`
    // is this CPU's `Pcpu`; the load is in bounds and the field immutable.
    unsafe {
        asm!(
            "mov {id:e}, dword ptr gs:[{off}]",
            id = out(reg) id,
            off = const offset_of!(Pcpu, cpuid),
            options(nostack, preserves_flags, readonly),
        )
    }
    id
}
