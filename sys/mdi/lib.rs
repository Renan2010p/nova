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

//! `mdi`: the **Machine-Dependent Interface**.
//!
//! This crate is the contract between the machine-independent kernel and
//! each architecture, and nothing else: traits and plain data types, no
//! code. It plays the role of the MD function prototypes that FreeBSD and
//! NetBSD scatter over `machine/cpufunc.h`, `vm/pmap.h`, `sys/cons.h`...,
//! gathered in one place so the whole MD surface can be read at once.
//!
//! Two directions of calls cross this boundary:
//!
//! * **Downcalls (MI → MD)**: the `framework` crate calls the traits
//!   below and wraps them in safe, SMP-aware APIs for the services. Every
//!   architecture crate implements all of them on a zero-sized `Machine`
//!   type; `framework` checks that at compile time, so a missing function
//!   is a build error, not a link error. Dispatch is static: no `dyn`, no
//!   generics in MI code.
//! * **Upcalls (MD → MI)**: when the MD bootstrap is done it calls
//!   [`MiStartup`], bound by the kernel binary with
//!   `framework::kernel_entry!`. Interrupts go to [`IntrUpcall`], defined
//!   by the framework. Both are link-time symbols whose types are checked
//!   against the aliases here, so MD code never imports MI code.
//!
//! Only the framework calls these traits; services never see them. Traits
//! whose misuse could break memory safety (page tables, context switch)
//! will be `unsafe trait`s with documented contracts.
//!
//! Adding a method here forces every architecture to implement it. Keep
//! the interface small and in MI terms; see docs/LAYERING.md.

#![no_std]
// `deny`, not `forbid`: declaring an `unsafe trait` (a contract the MD
// side must uphold) counts as unsafe code, and is the one thing allowed.
#![deny(unsafe_code)]

pub mod bootinfo;

pub use bootinfo::{BootInfo, Firmware};
pub use sys::physmem::PhysRange;

/// CPU control.
pub trait Cpu {
    /// Saved interrupt state, returned by [`Cpu::intr_disable`]. Opaque
    /// to MI code, so it can only be handed back to [`Cpu::intr_restore`].
    type IntrState: Copy;

    /// Disables interrupts on this CPU and returns the previous state.
    fn intr_disable() -> Self::IntrState;

    /// Restores the interrupt state saved by [`Cpu::intr_disable`].
    fn intr_restore(state: Self::IntrState);

    /// Enables interrupts on this CPU. The framework only calls it when no
    /// interrupt guard is held.
    fn intr_enable();

    /// Enables interrupts and sleeps until the next one (`cpu_idle`);
    /// returns with interrupts enabled. Same precondition as `intr_enable`.
    fn cpu_idle();

    /// Hint that the CPU is in a spin-wait loop.
    fn cpu_spinwait();

    /// Id of the executing CPU, `0..MAXCPU`; the boot CPU is 0. Must be
    /// valid from the first instruction of `mi_startup` on every CPU.
    fn curcpu() -> u32;

    /// Disables interrupts and stops this CPU for good.
    fn cpu_halt() -> !;

    /// Deliberately takes a fatal kernel page fault (a read of an
    /// unmapped address), to exercise the trap path. Never returns.
    fn cpu_selftest_fault() -> !;
}

/// The direct map: all of physical memory, mapped at a fixed offset in
/// kernel virtual space (FreeBSD's DMAP, `PHYS_TO_DMAP`).
///
/// # Safety
///
/// The framework builds safe abstractions (page frames, the kernel heap)
/// on these promises, so an implementation must guarantee that, from the
/// moment `mi_startup` runs and for the life of the kernel:
///
/// * every physical address below [`DirectMap::dmap_limit`] is mapped
///   read-write, write-back cacheable, at `phys_to_dmap(pa)`, on every CPU;
/// * the mapping never changes and is never aliased by another kernel
///   mapping that the MD layer hands out;
/// * `dmap_to_phys(phys_to_dmap(pa)) == pa`.
#[allow(unsafe_code)]
pub unsafe trait DirectMap {
    /// Kernel virtual address of physical address `pa`.
    /// Panics if `pa >= dmap_limit()`.
    fn phys_to_dmap(pa: u64) -> usize;

    /// Physical address of a direct-map virtual address.
    /// Panics if `va` is not in the direct map.
    fn dmap_to_phys(va: usize) -> u64;

    /// End of the physical range covered by the direct map.
    fn dmap_limit() -> u64;
}

/// The per-CPU periodic clock (FreeBSD's event timer).
pub trait Clock {
    /// Starts the periodic clock interrupt on this CPU at `hz` ticks per
    /// second. Each tick is delivered as [`Interrupt::Clock`].
    fn cpu_initclocks(hz: u32);
}

/// An interrupt delivered by the MD layer to the framework.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Interrupt {
    /// A tick of the per-CPU clock started by [`Clock::cpu_initclocks`].
    Clock,
}

/// Signature of the interrupt upcall, `framework::intr::__nova_intr`. The
/// MD trap code calls it with interrupts disabled, after acknowledging the
/// interrupt controller.
pub type IntrUpcall = fn(Interrupt);

/// Console available from the first instruction, before any driver is
/// attached (FreeBSD's early `cninit` path). Output only.
pub trait EarlyCons {
    /// Writes one byte, as is (no newline translation).
    fn putc(c: u8);
}

/// Everything an architecture must provide.
pub trait Machine: Cpu + EarlyCons + DirectMap + Clock {
    /// Machine name, `hw.machine` (e.g. `"amd64"`).
    const MACHINE: &'static str;
    /// Machine architecture, `hw.machine_arch` (e.g. `"amd64"`).
    const MACHINE_ARCH: &'static str;
    /// Base page size in bytes.
    const PAGE_SIZE: usize;
    /// Maximum number of CPUs (`MAXCPU`).
    const MAXCPU: usize;

    /// True when running under a hypervisor (FreeBSD's `vm_guest`).
    fn vm_guest() -> bool;
}

/// Signature of the MI entry point, `kern::init_main::mi_startup`. The MD
/// bootstrap calls it exactly once, on the boot CPU, with interrupts
/// disabled, the early console ready, traps handled and the direct map
/// in place.
pub type MiStartup = fn(&BootInfo<'_>) -> !;
