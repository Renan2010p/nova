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
//! | `swtch.rs`     | `amd64/cpu_switch.S`          | `cpu_switch`, `fork_trampoline` |
//! | `db_trace.rs`  | `amd64/db_trace.c`            | frame-pointer stack traces      |
//! | `vm_machdep.rs`| `x86/cpu_machdep.c`           | `cpu_reset`                     |
//! | `mp_machdep.rs`| `amd64/mp_machdep.c`, `x86/acpica/madt.c` | CPUs, AP start-up    |
//! | `mpboot.rs`    | `amd64/mpboot.S`              | AP real-mode trampoline         |
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

mod db_trace;
mod exception;
mod fpu;
mod locore;
mod machdep;
pub mod metadata;
mod mp_machdep;
mod mpboot;
pub mod param;
mod pcpu;
mod pmap;
mod segments;
mod swtch;
mod trap;
pub mod usermode;
mod vm_machdep;
pub mod vmparam;

use core::fmt;

use x86::cpufunc::{disable_intr, enable_intr, halt, ia32_pause, read_rflags, sti_hlt};
use x86::local_apic::{lapic_calibrate_timer, lapic_ipi_vectored, lapic_timer_periodic};
use x86::ns8250::{COM1, Ns8250};
use x86::psl::PSL_I;
use x86::{identcpu, tsc};

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

    fn delay(us: u32) {
        tsc::delay(us);
    }

    fn cpu_reset() -> ! {
        vm_machdep::cpu_reset()
    }

    fn stack_trace(f: &mut dyn FnMut(mdi::StackFrame) -> bool) {
        db_trace::stack_trace(f);
    }

    fn cpu_print_info(w: &mut dyn fmt::Write) -> fmt::Result {
        identcpu::print_cpu_info(w, tsc::tsc_freq())
    }
}

impl mdi::Context for Machine {
    type Pcb = swtch::Pcb;
    const PCB_INIT: swtch::Pcb = swtch::Pcb::INIT;

    unsafe fn cpu_thread_init(pcb: &mut swtch::Pcb, stack_top: usize, entry: extern "C" fn() -> !) {
        // SAFETY: forwarded to the caller (same contract).
        unsafe { swtch::cpu_thread_init(pcb, stack_top, entry) }
    }

    unsafe fn cpu_switch(old: *mut swtch::Pcb, new: *const swtch::Pcb) {
        // SAFETY: forwarded to the caller (same contract).
        unsafe { swtch::cpu_switch(old, new) }
    }
}

// SAFETY: `pmap_kenter`/`pmap_kremove` touch only PML4 slot 508
// (`VM_MIN_KERNEL_ADDRESS`), which nothing else in the MD layer uses, in
// the page tables shared by all CPUs; `pmap_kextract` walks the tables
// through the direct map and never dereferences `va` itself.
unsafe impl mdi::KernelMap for Machine {
    const KVA_MIN: usize = vmparam::VM_MIN_KERNEL_ADDRESS;
    const KVA_MAX: usize = vmparam::VM_MAX_KERNEL_MAP;

    unsafe fn pmap_kenter(va: usize, pa: u64, alloc: &mut dyn FnMut() -> Option<u64>) -> bool {
        // SAFETY: forwarded to the caller (same contract).
        unsafe { pmap::pmap_kenter(va, pa, alloc) }
    }

    unsafe fn pmap_kremove(va: usize) {
        // SAFETY: forwarded to the caller (same contract).
        unsafe { pmap::pmap_kremove(va) }
    }

    fn pmap_kextract(va: usize) -> Option<u64> {
        pmap::pmap_kextract(va)
    }
}

impl mdi::EarlyCons for Machine {
    fn putc(c: u8) {
        EARLY_CONS.putc(c);
    }

    fn checkc() -> Option<u8> {
        EARLY_CONS.getc()
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
    const ET_NAME: &'static str = "LAPIC";
    // FreeBSD gives the LAPIC timer quality 600 (100 when it stops in
    // deep C-states, which it does not in a VM or with ARAT).
    const ET_QUALITY: i32 = 600;

    fn cpu_initclocks(hz: u32) {
        let freq = lapic_calibrate_timer().unwrap_or_else(|e| panic!("local APIC timer: {e:?}"));
        lapic_timer_periodic(trap::APIC_TIMER_INT, freq / hz);
    }
}

impl mdi::Smp for Machine {
    fn cpu_mp_probe() -> u32 {
        mp_machdep::cpu_mp_probe()
    }

    fn cpu_topology() -> (u32, u32, u32) {
        mp_machdep::cpu_topology()
    }

    /// Double fault, NMI and machine check each run on their own stack
    /// (IST1, IST2, IST3).
    const AP_EXCEPTION_STACKS: usize = 3;

    fn cpu_hwid(cpuid: u32) -> u32 {
        mp_machdep::cpu_apic_id(cpuid)
    }

    fn cpu_mp_print_probe(w: &mut dyn fmt::Write) -> fmt::Result {
        mp_machdep::print_madt(w)
    }

    unsafe fn cpu_start_ap(cpuid: u32, stack_top: usize, exception_stack_tops: &[usize]) -> bool {
        let ist: [usize; 3] = exception_stack_tops
            .try_into()
            .expect("cpu_start_ap: need exactly 3 exception stacks");
        // SAFETY: forwarded to the caller (same contract).
        unsafe { mp_machdep::cpu_start_ap(cpuid, stack_top, ist) }
    }

    fn ipi_send(cpuid: u32, ipi: mdi::Ipi) {
        let vector = match ipi {
            mdi::Ipi::Ast => trap::IPI_BITMAP_VECTOR,
            mdi::Ipi::Stop => trap::IPI_STOP,
        };
        lapic_ipi_vectored(vector, mp_machdep::cpu_apic_id(cpuid));
    }
}

impl mdi::Machine for Machine {
    const MACHINE: &'static str = param::MACHINE;
    const MACHINE_ARCH: &'static str = param::MACHINE_ARCH;
    const PAGE_SIZE: usize = param::PAGE_SIZE;
    const MAXCPU: usize = param::MAXCPU;
    const KSTACK_PAGES: usize = param::KSTACK_PAGES;
    const VM_NFREEORDER: usize = param::VM_NFREEORDER;

    fn vm_guest() -> bool {
        identcpu::vm_guest()
    }

    /// `rdseed` (a true entropy source) if present, else `rdrand` (its
    /// conditioned output), retried as Intel recommends.
    fn cpu_random(out: &mut [u8]) -> bool {
        let source: fn() -> Option<u64> = if identcpu::has_rdseed() {
            x86::cpufunc::rdseed
        } else if identcpu::has_rdrand() {
            x86::cpufunc::rdrand
        } else {
            return false;
        };
        for chunk in out.chunks_mut(8) {
            let Some(v) = (0..100).find_map(|_| source()) else { return false };
            chunk.copy_from_slice(&v.to_le_bytes()[..chunk.len()]);
        }
        true
    }
}

// SAFETY: `usermode::cpu_user_enter` enters user mode only with user
// selectors, IOPL 0 and user-half %rip/%rsp, and every way back (system
// call, interrupt, exception) ends in its return; the pmap functions build
// page tables whose kernel half is the kernel's.
unsafe impl mdi::UserMode for Machine {
    type UserRegs = usermode::UserRegs;
    const VM_MAXUSER_ADDRESS: u64 = usermode::VM_MAXUSER_ADDRESS;

    fn pmap_pinit(alloc: &mut dyn FnMut() -> Option<u64>) -> Option<u64> {
        pmap::pmap_pinit(alloc)
    }

    unsafe fn pmap_enter_user(
        root: u64,
        va: u64,
        pa: u64,
        prot: mdi::Prot,
        alloc: &mut dyn FnMut() -> Option<u64>,
    ) -> bool {
        // SAFETY: forwarded to the caller (same contract).
        unsafe { pmap::pmap_enter_user(root, va, pa, prot, alloc) }
    }

    unsafe fn pmap_remove_user(root: u64, va: u64) -> Option<u64> {
        // SAFETY: forwarded to the caller (same contract).
        unsafe { pmap::pmap_remove_user(root, va) }
    }

    unsafe fn pmap_release(root: u64, free: &mut dyn FnMut(u64)) {
        // SAFETY: forwarded to the caller (same contract).
        unsafe { pmap::pmap_release(root, free) }
    }

    unsafe fn pmap_activate(root: u64) {
        // SAFETY: forwarded to the caller (same contract).
        unsafe { pmap::pmap_activate(root) }
    }

    unsafe fn cpu_user_enter(regs: &mut Self::UserRegs) -> mdi::UserExit {
        // SAFETY: forwarded to the caller (same contract).
        unsafe { usermode::cpu_user_enter(regs) }
    }

    const FPU_STATE_SIZE: usize = 512;

    fn fpu_save(buf: &mut [u8]) {
        fpu::fpu_save(buf);
    }

    fn fpu_load(state: Option<&[u8]>) {
        fpu::fpu_load(state);
    }
}
