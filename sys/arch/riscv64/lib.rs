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

//! `riscv64`: a **compile-only** port (FreeBSD's `sys/riscv` some day).
//!
//! It implements the whole `mdi` contract, and every operation panics: it
//! does not boot. Its only job is to make the kernel compile for a second
//! machine, so that `make check` proves what the layering rules promise:
//! no machine-dependent code has leaked into machine-independent code. If
//! an MI change breaks the riscv64 build, the change is wrong, not this
//! port. (NetBSD keeps its MI code honest the same way, by building it for
//! many machines.)
//!
//! Values that MI code reads at compile time (page size, `MAXCPU`, ...)
//! are FreeBSD/riscv's.

#![no_std]

use core::fmt;

use mdi::{Ipi, StackFrame};

/// The riscv64 implementation of the Machine-Dependent Interface.
pub struct Machine;

/// Saved interrupt state (`sstatus.SIE`). Private field: MI code cannot
/// forge one.
#[derive(Clone, Copy)]
pub struct IntrState(());

/// Saved context of a thread not running.
pub struct Pcb(());

fn todo(what: &str) -> ! {
    panic!("riscv64: {what} is not implemented (compile-only port)")
}

impl mdi::Cpu for Machine {
    type IntrState = IntrState;

    fn intr_disable() -> IntrState {
        todo("intr_disable")
    }
    fn intr_restore(_: IntrState) {
        todo("intr_restore")
    }
    fn intr_enable() {
        todo("intr_enable")
    }
    fn cpu_idle() {
        todo("cpu_idle")
    }
    fn cpu_spinwait() {
        core::hint::spin_loop();
    }
    fn curcpu() -> u32 {
        todo("curcpu")
    }
    fn cpu_halt() -> ! {
        todo("cpu_halt")
    }
    fn cpu_selftest_fault() -> ! {
        todo("cpu_selftest_fault")
    }
    fn delay(_: u32) {
        todo("DELAY")
    }
    fn cpu_reset() -> ! {
        todo("cpu_reset")
    }
    fn stack_trace(_: &mut dyn FnMut(StackFrame) -> bool) {}
    fn cpu_print_info(_: &mut dyn fmt::Write) -> fmt::Result {
        Ok(())
    }
}

impl mdi::EarlyCons for Machine {
    fn putc(_: u8) {}
    fn checkc() -> Option<u8> {
        None
    }
}

// SAFETY: no method returns: every promise of the contract holds
// vacuously.
unsafe impl mdi::DirectMap for Machine {
    fn phys_to_dmap(_: u64) -> usize {
        todo("PHYS_TO_DMAP")
    }
    fn dmap_to_phys(_: usize) -> u64 {
        todo("DMAP_TO_PHYS")
    }
    fn dmap_limit() -> u64 {
        todo("dmap_limit")
    }
}

// SAFETY: no mapping is ever made: every promise holds vacuously.
unsafe impl mdi::KernelMap for Machine {
    const KVA_MIN: usize = 0xffff_ffc0_0000_0000;
    const KVA_MAX: usize = 0xffff_ffd0_0000_0000;

    unsafe fn pmap_kenter(_: usize, _: u64, _: &mut dyn FnMut() -> Option<u64>) -> bool {
        todo("pmap_kenter")
    }
    unsafe fn pmap_kremove(_: usize) {
        todo("pmap_kremove")
    }
    fn pmap_kextract(_: usize) -> Option<u64> {
        None
    }
}

impl mdi::Clock for Machine {
    const ET_NAME: &'static str = "RISC-V timer";
    const ET_QUALITY: i32 = 1000;

    fn cpu_initclocks(_: u32) {
        todo("cpu_initclocks")
    }

    fn rtc_gettime() -> Option<u64> {
        None // TODO(riscv): the RTC device the device tree names
    }
}

impl mdi::Context for Machine {
    type Pcb = Pcb;
    const PCB_INIT: Pcb = Pcb(());

    unsafe fn cpu_thread_init(_: &mut Pcb, _: usize, _: extern "C" fn() -> !) {
        todo("cpu_thread_init")
    }
    unsafe fn cpu_switch(_: *mut Pcb, _: *const Pcb) {
        todo("cpu_switch")
    }
}

impl mdi::Smp for Machine {
    const AP_EXCEPTION_STACKS: usize = 0;

    fn cpu_mp_probe() -> u32 {
        1
    }
    fn cpu_topology() -> (u32, u32, u32) {
        (1, 1, 1)
    }
    fn cpu_hwid(_: u32) -> u32 {
        0
    }
    fn cpu_mp_print_probe(_: &mut dyn fmt::Write) -> fmt::Result {
        Ok(())
    }
    unsafe fn cpu_start_ap(_: u32, _: usize, _: &[usize]) -> bool {
        false
    }
    fn ipi_send(_: u32, _: Ipi) {
        todo("ipi_send")
    }
}

impl mdi::Machine for Machine {
    const MACHINE: &'static str = "riscv";
    const MACHINE_ARCH: &'static str = "riscv64";
    const PAGE_SIZE: usize = 4096;
    const MAXCPU: usize = 64;
    const KSTACK_PAGES: usize = 4;
    const VM_NFREEORDER: usize = 12;

    fn vm_guest() -> bool {
        false
    }

    fn cpu_random(_out: &mut [u8]) -> bool {
        false // TODO(riscv): the Zkr `seed` CSR
    }
}

/// The riscv64 user register file (`x1`..`x31` and `sepc`).
#[derive(Clone, Copy, Debug, Default)]
pub struct UserRegs {
    /// General registers `x1`..`x31` (`x[0]` is `ra`).
    pub x: [u64; 31],
    /// The user program counter.
    pub sepc: u64,
}

// SAFETY: nothing here enters user mode yet; every function panics.
unsafe impl mdi::UserMode for Machine {
    type UserRegs = UserRegs;
    /// `VM_MAXUSER_ADDRESS` for Sv39, as in FreeBSD.
    const VM_MAXUSER_ADDRESS: u64 = 0x0000_0040_0000_0000;

    fn pmap_pinit(_alloc: &mut dyn FnMut() -> Option<u64>) -> Option<u64> {
        todo("pmap_pinit")
    }

    unsafe fn pmap_enter_user(
        _root: u64,
        _va: u64,
        _pa: u64,
        _prot: mdi::Prot,
        _alloc: &mut dyn FnMut() -> Option<u64>,
    ) -> bool {
        todo("pmap_enter_user")
    }

    unsafe fn pmap_remove_user(_root: u64, _va: u64) -> Option<u64> {
        todo("pmap_remove_user")
    }

    unsafe fn pmap_release(_root: u64, _free: &mut dyn FnMut(u64)) {
        todo("pmap_release")
    }

    unsafe fn pmap_activate(_root: u64) {
        todo("pmap_activate")
    }

    unsafe fn cpu_user_enter(_regs: &mut Self::UserRegs) -> mdi::UserExit {
        todo("cpu_user_enter")
    }

    /// `f0`..`f31` and `fcsr`.
    const FPU_STATE_SIZE: usize = 33 * 8;

    fn fpu_save(_buf: &mut [u8]) {
        todo("fpu_save")
    }

    fn fpu_load(_state: Option<&[u8]>) {
        todo("fpu_load")
    }
}
