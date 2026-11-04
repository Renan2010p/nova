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

//! `framework`: the trusted core of the Nova **framekernel**.
//!
//! Nova is split in two:
//!
//! * the **framework** (this crate, plus `sys/arch/*` and `sys/libkern`)
//!   may use `unsafe`. It owns the *mechanisms*: CPUs, interrupts,
//!   per-CPU data, locks and wait queues, the clock interrupt, physical
//!   pages, the kernel heap, kernel stacks and thread switching, and as the
//!   kernel grows user address spaces, user memory access and device
//!   access (`bus_space`, `bus_dma`);
//! * the **services** (`vm`, `kern`, `fs`, `dev`, `compat`, ...) implement
//!   the *policies* on top of it in 100% safe Rust
//!   (`#![forbid(unsafe_code)]`).
//!
//! The invariant that makes this a framekernel: **every public API of
//! the framework is sound**. No safe code in a service, however buggy,
//! can cause undefined behaviour; memory-safety bugs can only live in the
//! framework, which is kept small and reviewed accordingly.
//!
//! The framework is also the only bridge to machine-dependent code
//! (the role of BSD's `<machine/...>`): it selects the `sys/arch` crate
//! for the target, checks at compile time that it implements the whole
//! `mdi` contract, and wraps it in MI, SMP-aware APIs. Services never name
//! an architecture.
//!
//! Which architecture that is, is said in exactly one file, `arch/mod.rs`.
//! Porting Nova to a new architecture means writing `sys/arch/<arch>` and
//! adding one line there and one to this crate's `Cargo.toml`.

#![no_std]


mod arch;

pub mod clock;
pub mod cons;
pub mod cpu;
pub mod intr;
pub mod mm;
pub mod pcpu;
pub mod smp;
pub mod sync;
pub mod task;
pub mod user;

/// Seals the framework's marker traits (`PinnedToCpu`): services can use
/// them as bounds but cannot implement them.
mod sealed {
    pub trait Sealed {}
}

pub use mdi::{BootInfo, Firmware, MiStartup, PhysRange, StackFrame};

/// The implementation of the `mdi` contract for this build.
type Md = arch::Machine;

const _: () = assert_implements_mdi::<Md>();
const fn assert_implements_mdi<M: mdi::Machine>() {}

/// `hw.machine` of this build.
pub const MACHINE: &str = <Md as mdi::Machine>::MACHINE;
/// `hw.machine_arch` of this build.
pub const MACHINE_ARCH: &str = <Md as mdi::Machine>::MACHINE_ARCH;
/// Base page size of this build.
pub const PAGE_SIZE: usize = <Md as mdi::Machine>::PAGE_SIZE;
/// Maximum number of CPUs this build supports.
pub const MAXCPU: usize = <Md as mdi::Machine>::MAXCPU;
/// Pages of each kernel thread stack.
pub const KSTACK_PAGES: usize = <Md as mdi::Machine>::KSTACK_PAGES;
/// Block orders of the physical page allocator.
pub const VM_NFREEORDER: usize = <Md as mdi::Machine>::VM_NFREEORDER;

static INITIALIZED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Brings up the framework's own services (the page allocator, hence the
/// heap) from the boot information. Called by [`kernel_entry!`] right
/// before `mi_startup`; services never need to call it.
///
/// # Panics
///
/// If called more than once.
#[doc(hidden)]
pub fn __init(boot: &BootInfo<'_>) {
    use core::sync::atomic::Ordering;
    assert!(
        !INITIALIZED.swap(true, Ordering::AcqRel),
        "framework initialized twice"
    );
    mm::vm_phys::init(boot.physavail);
    task::init();
    smp::init();
}

/// Binds the MI entry point that the MD bootstrap calls when it is done.
///
/// Use it exactly once, in the kernel binary:
///
/// ```ignore
/// framework::kernel_entry!(kern::init_main::mi_startup);
/// ```
///
/// The function must have the type [`MiStartup`]; anything else is a
/// compile error here, so the `extern` declaration on the MD side can rely
/// on it. The framework initializes itself (page allocator, heap) before
/// the function is called.
#[macro_export]
macro_rules! kernel_entry {
    ($mi_startup:path) => {
        #[doc(hidden)]
        #[allow(unsafe_code)]
        #[unsafe(no_mangle)]
        pub fn __nova_mi_startup(boot: &$crate::BootInfo<'_>) -> ! {
            const MI_STARTUP: $crate::MiStartup = $mi_startup;
            $crate::__init(boot);
            MI_STARTUP(boot)
        }
    };
}
