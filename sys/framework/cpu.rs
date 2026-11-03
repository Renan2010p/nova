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

//! CPU identity and interrupt control, SMP-aware from the start.
//!
//! Services never toggle interrupts directly: they hold an [`IntrGuard`],
//! which restores the previous state when dropped. Because there is no
//! raw "enable interrupts" call outside the framework, a live guard is a
//! *proof* that this CPU cannot be interrupted or preempted, which is what
//! [`PinnedToCpu`] and [`crate::pcpu::CpuLocal`] rely on.

use core::marker::PhantomData;
use core::sync::atomic::{AtomicU32, Ordering};

use mdi::Cpu;

use crate::{MAXCPU, Md};

/// Identifier of a CPU, always below [`MAXCPU`] (FreeBSD's `cpuid`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CpuId(u32);

impl CpuId {
    /// The boot processor.
    pub const BSP: CpuId = CpuId(0);

    /// The id as an array index.
    pub const fn index(self) -> usize {
        self.0 as usize
    }

    /// The id as a number.
    pub const fn as_u32(self) -> u32 {
        self.0
    }
}

/// The CPU executing this code (`curcpu`).
///
/// Without a [`PinnedToCpu`] guard the answer may be stale by the time it
/// is used, once the scheduler can migrate threads. Use it for reporting;
/// use [`crate::pcpu::CpuLocal`] for per-CPU state.
pub fn curcpu() -> CpuId {
    let id = Md::curcpu();
    assert!((id as usize) < MAXCPU, "MD curcpu() out of range");
    CpuId(id)
}

/// Number of CPUs running (`mp_ncpus`). 1 until the APs are started.
static MP_NCPUS: AtomicU32 = AtomicU32::new(1);

/// Number of CPUs running (`mp_ncpus`).
pub fn mp_ncpus() -> u32 {
    MP_NCPUS.load(Ordering::Relaxed)
}

/// Interrupts are disabled on this CPU while this guard lives. It is
/// `!Send`: it describes the CPU it was created on.
pub struct IntrGuard {
    saved: <Md as Cpu>::IntrState,
    _not_send: PhantomData<*const ()>,
}

/// Disables interrupts on this CPU until the guard is dropped
/// (`intr_disable` / `intr_restore`). Guards nest.
pub fn intr_guard() -> IntrGuard {
    IntrGuard {
        saved: Md::intr_disable(),
        _not_send: PhantomData,
    }
}

impl Drop for IntrGuard {
    fn drop(&mut self) {
        Md::intr_restore(self.saved);
    }
}

/// Spin-wait hint (`cpu_spinwait`).
#[inline(always)]
pub fn cpu_spinwait() {
    Md::cpu_spinwait()
}

/// Disables interrupts and stops this CPU for good.
pub fn cpu_halt() -> ! {
    Md::cpu_halt()
}

/// Evidence that the current code cannot leave this CPU: interrupts are
/// off, so neither an interrupt handler nor the scheduler can run here.
///
/// Sealed: only framework guards can implement it.
pub trait PinnedToCpu: crate::sealed::Sealed {}

impl crate::sealed::Sealed for IntrGuard {}
impl PinnedToCpu for IntrGuard {}
