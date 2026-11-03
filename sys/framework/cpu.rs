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
//! Services disable interrupts only by holding an [`IntrGuard`], which
//! restores the previous state when dropped. The framework counts the
//! guards alive on each CPU, and [`intr_enable`] and [`cpu_idle`] refuse
//! to enable interrupts while any is held. So a live guard is a *proof*
//! that this CPU cannot be interrupted or preempted, which is what
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

    /// The CPU with id `id`.
    ///
    /// # Panics
    ///
    /// If `id >= MAXCPU`.
    pub const fn from_u32(id: u32) -> CpuId {
        assert!((id as usize) < MAXCPU, "CpuId: out of range");
        CpuId(id)
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

/// Counts one more CPU as running (an AP came up).
pub(crate) fn mp_ncpus_inc() {
    MP_NCPUS.fetch_add(1, Ordering::Relaxed);
}

/// Interrupt guards alive on each CPU. Only a CPU touches its own
/// counter, always with interrupts disabled.
static INTR_DEPTH: [AtomicU32; MAXCPU] = [const { AtomicU32::new(0) }; MAXCPU];

/// Interrupts are disabled on this CPU while this guard lives. It is
/// `!Send`: it describes the CPU it was created on.
pub struct IntrGuard {
    saved: <Md as Cpu>::IntrState,
    _not_send: PhantomData<*const ()>,
}

/// Disables interrupts on this CPU until the guard is dropped
/// (`intr_disable` / `intr_restore`). Guards nest.
pub fn intr_guard() -> IntrGuard {
    let saved = Md::intr_disable();
    // Interrupts are off: this cannot migrate between the two lines.
    INTR_DEPTH[curcpu().index()].fetch_add(1, Ordering::Relaxed);
    IntrGuard {
        saved,
        _not_send: PhantomData,
    }
}

impl Drop for IntrGuard {
    fn drop(&mut self) {
        INTR_DEPTH[curcpu().index()].fetch_sub(1, Ordering::Relaxed);
        Md::intr_restore(self.saved);
    }
}

/// Interrupt guards alive on this CPU.
pub(crate) fn intr_depth() -> u32 {
    INTR_DEPTH[curcpu().index()].load(Ordering::Relaxed)
}

/// A new thread's first code takes over the count of the guard that
/// switched to it (alive on the previous thread's stack) and enables
/// interrupts. See `task`'s switch invariant.
pub(crate) fn switch_entry_release() {
    let prev = INTR_DEPTH[curcpu().index()].fetch_sub(1, Ordering::Relaxed);
    assert_eq!(prev, 1, "switch_entry_release: unbalanced interrupt guards");
    Md::intr_enable();
}

/// Panics with `what` if an interrupt guard is held on this CPU.
pub(crate) fn assert_no_guard(what: &str) {
    let depth = INTR_DEPTH[curcpu().index()].load(Ordering::Relaxed);
    assert!(
        depth == 0,
        "{what}: called while {depth} interrupt guard(s) are held"
    );
}

/// Enables interrupts on this CPU (`enable_intr`). Used once by the MI
/// start-up code, when the kernel is ready to take interrupts.
///
/// # Panics
///
/// If an [`IntrGuard`] is alive on this CPU: enabling interrupts would
/// break the guarantee the guard gives.
pub fn intr_enable() {
    assert_no_guard("intr_enable");
    Md::intr_enable()
}

/// Enables interrupts and sleeps until the next one (`cpu_idle`). Returns
/// with interrupts enabled.
///
/// # Panics
///
/// If an [`IntrGuard`] is alive on this CPU, as for [`intr_enable`].
pub fn cpu_idle() {
    assert_no_guard("cpu_idle");
    Md::cpu_idle()
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

/// Deliberately takes a fatal kernel page fault, to check that traps are
/// caught and reported. Sound: the fault panics, nothing is corrupted.
pub fn cpu_selftest_fault() -> ! {
    Md::cpu_selftest_fault()
}

/// True when running under a hypervisor (FreeBSD's `vm_guest`).
pub fn vm_guest() -> bool {
    <Md as mdi::Machine>::vm_guest()
}

/// Busy-waits at least `us` microseconds (`DELAY`), even with interrupts
/// off.
pub fn delay(us: u32) {
    Md::delay(us)
}

/// Resets the machine (`cpu_reset`).
pub fn cpu_reset() -> ! {
    Md::cpu_reset()
}

/// Calls `f` with each frame of the caller's stack, innermost first,
/// until it returns false, crossing trap frames (`db_backtrace`).
pub fn stack_trace(f: &mut dyn FnMut(mdi::StackFrame) -> bool) {
    Md::stack_trace(f)
}

/// Writes the boot CPU's identification in FreeBSD's format
/// (`printcpuinfo`).
pub fn print_cpu_info(w: &mut dyn core::fmt::Write) -> core::fmt::Result {
    Md::cpu_print_info(w)
}

/// Evidence that the current code cannot leave this CPU: interrupts are
/// off, so neither an interrupt handler nor the scheduler can run here.
///
/// Sealed: only framework guards can implement it.
pub trait PinnedToCpu: crate::sealed::Sealed {}

impl crate::sealed::Sealed for IntrGuard {}
impl PinnedToCpu for IntrGuard {}
