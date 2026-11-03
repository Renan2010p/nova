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

//! Trap and interrupt dispatch (FreeBSD's `amd64/trap.c` and the APIC
//! vector handlers of `x86/x86/local_apic.c`).
//!
//! Interrupts are acknowledged here and handed to the framework through
//! the `mdi::IntrUpcall`. Exceptions in kernel mode are fatal for now:
//! the kernel is fully mapped, so a kernel page fault is a bug. They are
//! reported exactly like FreeBSD's `trap_fatal` does. User-mode traps and
//! the `vm_fault` upcall come with user mode.

use core::fmt::{self, Write};

use x86::cpufunc::{rcr2, rdmsr};
use x86::local_apic::{lapic_eoi, lapic_id};
use x86::psl::{PSL_I, PSL_RF, PSL_T};
use x86::specialreg::{MSR_FSBASE, MSR_GSBASE, MSR_KGSBASE};

use crate::EARLY_CONS;

/// Vector of the local APIC timer (FreeBSD's `APIC_TIMER_INT`).
pub const APIC_TIMER_INT: u8 = 0xef;
/// Vector of spurious local APIC interrupts (`APIC_SPURIOUS_INT`).
pub const APIC_SPURIOUS_INT: u8 = 0xff;
/// Vector of the IPIs that carry a bitmap (`IPI_BITMAP_VECTOR`); Nova
/// sends only `IPI_AST` on it.
pub const IPI_BITMAP_VECTOR: u8 = 0xf8;
/// Vector of `IPI_STOP`.
pub const IPI_STOP: u8 = 0xf9;

/// `tf_trapno` flag for interrupts: the low byte is the vector.
pub(crate) const T_INTR: u64 = 0x100;

// FreeBSD's trap codes (`x86/include/trap.h`).
const T_PAGEFLT: u64 = 12;
const T_DOUBLEFLT: u64 = 23;

// Page fault error code bits (`PGEX_*`).
const PGEX_P: u64 = 0x01;
const PGEX_W: u64 = 0x02;
const PGEX_U: u64 = 0x04;
const PGEX_I: u64 = 0x10;

/// Registers saved on kernel entry, in the order `alltraps` leaves them
/// on the stack (lowest address first).
#[repr(C)]
#[derive(Debug)]
#[allow(missing_docs)]
pub struct TrapFrame {
    pub tf_rax: u64,
    pub tf_rbx: u64,
    pub tf_rcx: u64,
    pub tf_rdx: u64,
    pub tf_rsi: u64,
    pub tf_rdi: u64,
    pub tf_rbp: u64,
    pub tf_r8: u64,
    pub tf_r9: u64,
    pub tf_r10: u64,
    pub tf_r11: u64,
    pub tf_r12: u64,
    pub tf_r13: u64,
    pub tf_r14: u64,
    pub tf_r15: u64,
    /// FreeBSD `T_*` code, or `T_INTR | vector`.
    pub tf_trapno: u64,
    /// CPU error code, or 0.
    pub tf_err: u64,
    pub tf_rip: u64,
    pub tf_cs: u64,
    pub tf_rflags: u64,
    pub tf_rsp: u64,
    pub tf_ss: u64,
}

unsafe extern "Rust" {
    /// The framework's interrupt entry (`mdi::IntrUpcall`).
    fn __nova_intr(intr: mdi::Interrupt);
    /// The framework's interrupt exit (`mdi::IntrExitUpcall`).
    fn __nova_intr_exit();
}

/// Called by `alltraps` with interrupts disabled.
pub(crate) extern "C" fn trap(frame: &mut TrapFrame) {
    if frame.tf_trapno & T_INTR != 0 {
        match frame.tf_trapno as u8 {
            APIC_TIMER_INT => {
                lapic_eoi();
                // SAFETY: both are defined once by the framework with the
                // types of `mdi::IntrUpcall` and `mdi::IntrExitUpcall`
                // (checked there); interrupts are off, and we are on the
                // interrupted thread's stack, as the exit upcall requires.
                unsafe {
                    __nova_intr(mdi::Interrupt::Clock);
                    __nova_intr_exit();
                }
            }
            IPI_BITMAP_VECTOR | IPI_STOP => {
                lapic_eoi();
                let ipi = if frame.tf_trapno as u8 == IPI_STOP {
                    mdi::Ipi::Stop
                } else {
                    mdi::Ipi::Ast
                };
                // SAFETY: as for the clock interrupt above.
                unsafe {
                    __nova_intr(mdi::Interrupt::Ipi(ipi));
                    __nova_intr_exit();
                }
            }
            // Spurious interrupts must not be acknowledged.
            APIC_SPURIOUS_INT => {}
            _ => trap_fatal(frame),
        }
        return;
    }
    if frame.tf_trapno == T_DOUBLEFLT {
        dblfault_handler(frame);
    }
    trap_fatal(frame)
}

/// FreeBSD's `trap_msg[]`.
fn trap_msg(trapno: u64) -> &'static str {
    match trapno {
        1 => "privileged instruction fault",
        3 => "breakpoint instruction fault",
        6 => "arithmetic trap",
        9 => "general protection fault",
        10 => "debug exception",
        12 => "page fault",
        14 => "alignment fault",
        18 => "integer divide fault",
        19 => "non-maskable interrupt trap",
        20 => "overflow trap",
        21 => "FPU bounds check fault",
        22 => "FPU device not available",
        23 => "double fault",
        24 => "FPU operand fetch fault",
        25 => "invalid TSS fault",
        26 => "segment not present fault",
        27 => "stack fault",
        28 => "machine check trap",
        29 => "SIMD floating-point exception",
        30 => "reserved (unknown) fault",
        _ => "unknown/reserved trap",
    }
}

/// Raw output to the early console. A fatal trap may have interrupted the
/// MI console layer, so it is bypassed, as FreeBSD's `trap_fatal` writes
/// before `panic` takes over.
struct Cons;

impl Write for Cons {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for b in s.bytes() {
            if b == b'\n' {
                EARLY_CONS.putc(b'\r');
            }
            EARLY_CONS.putc(b);
        }
        Ok(())
    }
}

/// A double fault: usually a kernel stack overflow (the page fault on the
/// guard page could not be delivered on the same stack). It runs on its
/// own IST stack; reported like FreeBSD's `dblfault_handler`.
fn dblfault_handler(frame: &TrapFrame) -> ! {
    let w = &mut Cons;
    // SAFETY: these MSRs exist on every amd64 CPU; reading them has no
    // side effects.
    let (fsbase, gsbase, kgsbase) =
        unsafe { (rdmsr(MSR_FSBASE), rdmsr(MSR_GSBASE), rdmsr(MSR_KGSBASE)) };
    let _ = write!(
        w,
        "\nFatal double fault\n\
         rip {:#x} rsp {:#x} rbp {:#x}\n\
         rax {:#x} rdx {:#x} rbx {:#x}\n\
         rcx {:#x} rsi {:#x} rdi {:#x}\n\
         r8 {:#x} r9 {:#x} r10 {:#x}\n\
         r11 {:#x} r12 {:#x} r13 {:#x}\n\
         r14 {:#x} r15 {:#x} rflags {:#x}\n\
         cs {:#x} ss {:#x}\n\
         fsbase {:#x} gsbase {:#x} kgsbase {:#x}\n",
        frame.tf_rip,
        frame.tf_rsp,
        frame.tf_rbp,
        frame.tf_rax,
        frame.tf_rdx,
        frame.tf_rbx,
        frame.tf_rcx,
        frame.tf_rsi,
        frame.tf_rdi,
        frame.tf_r8,
        frame.tf_r9,
        frame.tf_r10,
        frame.tf_r11,
        frame.tf_r12,
        frame.tf_r13,
        frame.tf_r14,
        frame.tf_r15,
        frame.tf_rflags,
        frame.tf_cs,
        frame.tf_ss,
        fsbase,
        gsbase,
        kgsbase,
    );
    let _ = writeln!(
        w,
        "cpuid = {}; apic id = {:02x}",
        crate::pcpu::curcpu(),
        lapic_id()
    );
    panic!("double fault");
}

/// Reports a fatal trap like FreeBSD's `trap_diag`, then panics with the
/// trap's name, as `trap_fatal` does.
fn trap_fatal(frame: &TrapFrame) -> ! {
    let _ = trap_diag(frame, &mut Cons);
    if frame.tf_trapno & T_INTR != 0 {
        panic!("unexpected interrupt on vector {}", frame.tf_trapno as u8);
    }
    panic!("{}", trap_msg(frame.tf_trapno));
}

fn trap_diag(frame: &TrapFrame, w: &mut impl Write) -> fmt::Result {
    let user = frame.tf_cs & 3 != 0;
    let mode = if user { "user" } else { "kernel" };
    writeln!(
        w,
        "\n\nFatal trap {}: {} while in {} mode",
        frame.tf_trapno,
        trap_msg(frame.tf_trapno),
        mode
    )?;
    writeln!(
        w,
        "cpuid = {}; apic id = {:02x}",
        crate::pcpu::curcpu(),
        lapic_id()
    )?;
    if frame.tf_trapno == T_PAGEFLT {
        let code = frame.tf_err;
        writeln!(w, "fault virtual address\t= {:#x}", rcr2())?;
        writeln!(
            w,
            "fault code\t\t= {} {} {}, {}",
            if code & PGEX_U != 0 {
                "user"
            } else {
                "supervisor"
            },
            if code & PGEX_W != 0 { "write" } else { "read" },
            if code & PGEX_I != 0 {
                "instruction"
            } else {
                "data"
            },
            if code & PGEX_P != 0 {
                "protection violation"
            } else {
                "page not present"
            },
        )?;
    }
    writeln!(
        w,
        "instruction pointer\t= {:#x}:{:#x}",
        frame.tf_cs, frame.tf_rip
    )?;
    writeln!(
        w,
        "stack pointer\t        = {:#x}:{:#x}",
        frame.tf_ss, frame.tf_rsp
    )?;
    writeln!(
        w,
        "frame pointer\t        = {:#x}:{:#x}",
        frame.tf_ss, frame.tf_rbp
    )?;
    write!(w, "processor eflags\t= ")?;
    if frame.tf_rflags & PSL_T != 0 {
        write!(w, "trace trap, ")?;
    }
    if frame.tf_rflags & PSL_I != 0 {
        write!(w, "interrupt enabled, ")?;
    }
    if frame.tf_rflags & PSL_RF != 0 {
        write!(w, "resume, ")?;
    }
    writeln!(w, "IOPL = {}", (frame.tf_rflags >> 12) & 3)?;
    writeln!(
        w,
        "rdi: {:016x} rsi: {:016x} rdx: {:016x}",
        frame.tf_rdi, frame.tf_rsi, frame.tf_rdx
    )?;
    writeln!(
        w,
        "rcx: {:016x}  r8: {:016x}  r9: {:016x}",
        frame.tf_rcx, frame.tf_r8, frame.tf_r9
    )?;
    writeln!(
        w,
        "rax: {:016x} rbx: {:016x} rbp: {:016x}",
        frame.tf_rax, frame.tf_rbx, frame.tf_rbp
    )?;
    writeln!(
        w,
        "r10: {:016x} r11: {:016x} r12: {:016x}",
        frame.tf_r10, frame.tf_r11, frame.tf_r12
    )?;
    writeln!(
        w,
        "r13: {:016x} r14: {:016x} r15: {:016x}",
        frame.tf_r13, frame.tf_r14, frame.tf_r15
    )?;
    writeln!(w, "trap number\t\t= {}", frame.tf_trapno)
}
