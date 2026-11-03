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
//!   [`MiStartup`], the MI entry point. The kernel binary binds it with
//!   `framework::kernel_entry!`. Trap, interrupt, syscall and AP start-up
//!   upcalls will be added the same way (a `KernelHooks` trait registered
//!   by `mi_startup`), so MD code never imports MI code.
//!
//! Only the framework calls these traits; services never see them. Traits
//! whose misuse could break memory safety (page tables, context switch)
//! will be `unsafe trait`s with documented contracts.
//!
//! Adding a method here forces every architecture to implement it. Keep
//! the interface small and in MI terms; see docs/LAYERING.md.

#![no_std]
#![forbid(unsafe_code)]

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

    /// Hint that the CPU is in a spin-wait loop.
    fn cpu_spinwait();

    /// Id of the executing CPU, `0..MAXCPU`; the boot CPU is 0. Must be
    /// valid from the first instruction of `mi_startup` on every CPU.
    fn curcpu() -> u32;

    /// Disables interrupts and stops this CPU for good.
    fn cpu_halt() -> !;
}

/// Console available from the first instruction, before any driver is
/// attached (FreeBSD's early `cninit` path). Output only.
pub trait EarlyCons {
    /// Writes one byte, as is (no newline translation).
    fn putc(c: u8);
}

/// Everything an architecture must provide.
pub trait Machine: Cpu + EarlyCons {
    /// Machine name, `hw.machine` (e.g. `"amd64"`).
    const MACHINE: &'static str;
    /// Machine architecture, `hw.machine_arch` (e.g. `"amd64"`).
    const MACHINE_ARCH: &'static str;
    /// Base page size in bytes.
    const PAGE_SIZE: usize;
    /// Maximum number of CPUs (`MAXCPU`).
    const MAXCPU: usize;
}

/// Signature of the MI entry point, `kern::init_main::mi_startup`. The MD
/// bootstrap calls it exactly once, on the boot CPU, with interrupts
/// disabled and the early console ready.
pub type MiStartup = fn(&BootInfo<'_>) -> !;
