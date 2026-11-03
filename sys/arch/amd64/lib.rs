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

//! `amd64`: the x86_64 port of Nova (FreeBSD's `sys/amd64`).
//!
//! | File           | FreeBSD counterpart           | Role                            |
//! |----------------|-------------------------------|---------------------------------|
//! | `locore.rs`    | `amd64/locore.S`              | `btext`, the kernel entry       |
//! | `machdep.rs`   | `amd64/machdep.c`             | `hammer_time`, memory map       |
//! | `segments.rs`  | `x86/segments.h`, `machdep.c` | GDT, TSS, IDT, IST stacks       |
//! | `exception.rs` | `amd64/exception.S`           | vector stubs, `alltraps`        |
//! | `trap.rs`      | `amd64/trap.c`                | `trap`, `trap_fatal`            |
//! | `pmap.rs`      | `amd64/pmap.c`                | kernel page tables, direct map  |
//! | `metadata.rs`  | `x86/include/metadata.h`      | MD preload record types         |
//! | `param.rs`     | `amd64/include/param.h`       | page size, machine names        |
//! | `pcpu.rs`      | `amd64/include/pcpu.h`        | per-CPU area through `%gs`      |
//! | `vmparam.rs`   | `amd64/include/vmparam.h`     | `KERNBASE`, DMAP, kernel layout |
//!
//! The public face of this crate is [`Machine`], its implementation of the
//! `mdi` contract. MI code never names this crate; only `framework` does.
//! This crate never names MI crates; it calls back into MI code only
//! through the `mdi` upcalls.

#![no_std]

mod exception;
mod locore;
mod machdep;
pub mod metadata;
pub mod param;
mod pcpu;
mod pmap;
mod segments;
mod trap;
pub mod vmparam;

use x86::cpufunc::{disable_intr, enable_intr, halt, ia32_pause, read_rflags, sti_hlt};
use x86::identcpu;
use x86::local_apic::{lapic_calibrate_timer, lapic_timer_periodic};
use x86::ns8250::{COM1, Ns8250};
use x86::psl::PSL_I;

/// The early console on COM1, programmed by `hammer_time`.
// SAFETY: COM1 is the PC's first serial port (or nothing). Only this
// static drives it until the MI UART driver takes over the console.
static EARLY_CONS: Ns8250 = unsafe { Ns8250::new(COM1) };

/// The amd64 implementation of the Machine-Dependent Interface.
pub struct Machine;

/// Saved `%rflags`. The field is private, so MI code cannot forge one.
#[derive(Clone, Copy)]
pub struct IntrState(u64);

impl mdi::Cpu for Machine {
    type IntrState = IntrState;

    fn intr_disable() -> IntrState {
        let rflags = read_rflags();
        disable_intr();
        IntrState(rflags)
    }

    fn intr_restore(state: IntrState) {
        if state.0 & PSL_I != 0 {
            enable_intr();
        }
    }

    fn intr_enable() {
        enable_intr();
    }

    fn cpu_idle() {
        sti_hlt();
    }

    fn cpu_spinwait() {
        ia32_pause();
    }

    fn curcpu() -> u32 {
        pcpu::curcpu()
    }

    fn cpu_halt() -> ! {
        loop {
            disable_intr();
            halt();
        }
    }

    fn cpu_selftest_fault() -> ! {
        // SAFETY: reads one byte from an address that `pmap_bootstrap` left
        // unmapped, which raises a page fault; `trap` reports it and panics,
        // so this never returns. Done in asm so that no Rust-level pointer
        // dereference (which would be undefined behaviour) is involved.
        unsafe {
            core::arch::asm!("mov {0}, byte ptr [{1}]", out(reg_byte) _, in(reg) 0xdead_beef_usize);
        }
        unreachable!("selftest fault returned")
    }
}

impl mdi::EarlyCons for Machine {
    fn putc(c: u8) {
        EARLY_CONS.putc(c);
    }
}

// SAFETY: `pmap_bootstrap` maps every physical address below
// `dmap_limit()` read-write, write-back, at DMAP_MIN_ADDRESS + pa, before
// `mi_startup` runs, with the same page tables on every CPU; nothing ever
// changes or removes those entries, and no other kernel mapping is handed
// out over them. The two conversions are exact inverses.
unsafe impl mdi::DirectMap for Machine {
    fn phys_to_dmap(pa: u64) -> usize {
        pmap::phys_to_dmap(pa)
    }

    fn dmap_to_phys(va: usize) -> u64 {
        pmap::dmap_to_phys(va)
    }

    fn dmap_limit() -> u64 {
        pmap::dmap_limit()
    }
}

impl mdi::Clock for Machine {
    fn cpu_initclocks(hz: u32) {
        let freq = lapic_calibrate_timer().unwrap_or_else(|e| panic!("local APIC timer: {e:?}"));
        lapic_timer_periodic(trap::APIC_TIMER_INT, freq / hz);
    }
}

impl mdi::Machine for Machine {
    const MACHINE: &'static str = param::MACHINE;
    const MACHINE_ARCH: &'static str = param::MACHINE_ARCH;
    const PAGE_SIZE: usize = param::PAGE_SIZE;
    const MAXCPU: usize = param::MAXCPU;

    fn vm_guest() -> bool {
        identcpu::vm_guest()
    }
}
