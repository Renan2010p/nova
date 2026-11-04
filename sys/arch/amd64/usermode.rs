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

//! User mode (the amd64 side of FreeBSD's `doreti` to user,
//! `fast_syscall` and user traps), as a function call.
//!
//! `cpu_user_enter(regs)` runs the current thread in user mode and returns
//! when the program comes back to the kernel:
//!
//! * `nova_user_enter` saves the kernel's callee-saved registers on the
//!   thread's stack, records that stack pointer (`PC_KERN_RSP`) and the
//!   `regs` pointer (`PC_USER_REGS`) in the per-CPU area, points
//!   `TSS.rsp0` just below it, loads the user registers and `iretq`s;
//! * `syscall` lands in `Xfast_syscall`, which stores the user registers
//!   into `*regs` and "returns" from `nova_user_enter` with code 0;
//! * an interrupt or exception from user mode lands in `alltraps`, which
//!   sees the user `%cs`, and `user_trap` stores the registers, handles an
//!   interrupt as usual (it may switch threads), and returns 1 or 2.
//!
//! The kernel never dereferences a user address: `copyin`/`copyout` work
//! on the pages the framework owns. Nothing here knows any ABI: which
//! register holds what for a system call is the ABI's business.

use core::arch::global_asm;
use core::mem::offset_of;

use mdi::{FaultKind, UserExit, UserFault};
use x86::cpufunc::{rcr2, rdmsr, wrmsr};
use x86::psl::user_rflags;
use x86::segments::{SEL_KPL, SEL_UPL, gsel};
use x86::specialreg::{EFER_SCE, MSR_EFER, MSR_LSTAR, MSR_SF_MASK, MSR_STAR};

use crate::pcpu::{PC_KERN_RSP, PC_SCRATCH, PC_TSS_RSP0, PC_USER_REGS, pcpu_self};
use crate::segments::{GCODE_SEL, GUCODE_SEL, GUCODE32_SEL, GUDATA_SEL};
use crate::trap::{T_INTR, TrapFrame, trap};

/// The user register file of amd64: what a program sees in its
/// registers. ABIs read and write it in their `arch/amd64/` half.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
#[allow(missing_docs)]
pub struct UserRegs {
    pub rax: u64,
    pub rbx: u64,
    pub rcx: u64,
    pub rdx: u64,
    pub rsi: u64,
    pub rdi: u64,
    pub rbp: u64,
    pub r8: u64,
    pub r9: u64,
    pub r10: u64,
    pub r11: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
    pub rip: u64,
    pub rflags: u64,
    pub rsp: u64,
    /// Filled in on a fault: FreeBSD's `T_*` code, error code, address.
    trapno: u64,
    err: u64,
    fault_addr: u64,
}

/// `nova_user_enter`'s return codes.
const EXIT_SYSCALL: u64 = 0;
const EXIT_INTERRUPT: u64 = 1;
const EXIT_FAULT: u64 = 2;

/// First address above user space (`VM_MAXUSER_ADDRESS_LA48`).
pub(crate) const VM_MAXUSER_ADDRESS: u64 = 0x0000_8000_0000_0000;

macro_rules! off {
    ($f:ident) => {
        offset_of!(UserRegs, $f)
    };
}

global_asm!(
    ".section .text",
    // nova_user_enter(rdi = &mut UserRegs) -> rax = exit code
    ".globl nova_user_enter",
    ".type nova_user_enter, @function",
    "nova_user_enter:",
    "    push rbp",
    "    push rbx",
    "    push r12",
    "    push r13",
    "    push r14",
    "    push r15",
    "    mov gs:[{pc_kern_rsp}], rsp",
    "    mov gs:[{pc_user_regs}], rdi",
    // Traps from user mode push their frame below the saved context.
    "    mov rax, gs:[{pc_tss_rsp0}]",
    "    mov [rax], rsp",
    "    push {udata}",
    "    push qword ptr [rdi + {o_rsp}]",
    "    push qword ptr [rdi + {o_rflags}]",
    "    push {ucode}",
    "    push qword ptr [rdi + {o_rip}]",
    "    mov rax, [rdi + {o_rax}]",
    "    mov rbx, [rdi + {o_rbx}]",
    "    mov rcx, [rdi + {o_rcx}]",
    "    mov rdx, [rdi + {o_rdx}]",
    "    mov rsi, [rdi + {o_rsi}]",
    "    mov rbp, [rdi + {o_rbp}]",
    "    mov r8, [rdi + {o_r8}]",
    "    mov r9, [rdi + {o_r9}]",
    "    mov r10, [rdi + {o_r10}]",
    "    mov r11, [rdi + {o_r11}]",
    "    mov r12, [rdi + {o_r12}]",
    "    mov r13, [rdi + {o_r13}]",
    "    mov r14, [rdi + {o_r14}]",
    "    mov r15, [rdi + {o_r15}]",
    "    mov rdi, [rdi + {o_rdi}]",
    "    swapgs",
    "    iretq",
    ".size nova_user_enter, . - nova_user_enter",
    "",
    // nova_user_return(rdi = saved kernel rsp, rsi = exit code) -> !
    ".globl nova_user_return",
    ".type nova_user_return, @function",
    "nova_user_return:",
    "    mov rsp, rdi",
    "    mov rax, rsi",
    "    pop r15",
    "    pop r14",
    "    pop r13",
    "    pop r12",
    "    pop rbx",
    "    pop rbp",
    "    ret",
    ".size nova_user_return, . - nova_user_return",
    "",
    // syscall from user mode: interrupts are masked (MSR_SF_MASK).
    ".globl Xfast_syscall",
    ".type Xfast_syscall, @function",
    "Xfast_syscall:",
    "    swapgs",
    "    mov gs:[{pc_scratch}], rsp",
    "    mov rsp, gs:[{pc_user_regs}]",
    "    mov [rsp + {o_rax}], rax",
    "    mov [rsp + {o_rbx}], rbx",
    "    mov [rsp + {o_rcx}], rcx",
    "    mov [rsp + {o_rdx}], rdx",
    "    mov [rsp + {o_rsi}], rsi",
    "    mov [rsp + {o_rdi}], rdi",
    "    mov [rsp + {o_rbp}], rbp",
    "    mov [rsp + {o_r8}], r8",
    "    mov [rsp + {o_r9}], r9",
    "    mov [rsp + {o_r10}], r10",
    "    mov [rsp + {o_r11}], r11",
    "    mov [rsp + {o_r12}], r12",
    "    mov [rsp + {o_r13}], r13",
    "    mov [rsp + {o_r14}], r14",
    "    mov [rsp + {o_r15}], r15",
    "    mov [rsp + {o_rip}], rcx",    // syscall saved the user %rip in %rcx
    "    mov [rsp + {o_rflags}], r11", // and %rflags in %r11
    "    mov rax, gs:[{pc_scratch}]",
    "    mov [rsp + {o_rsp}], rax",
    "    mov rdi, gs:[{pc_kern_rsp}]",
    "    mov esi, {exit_syscall}",
    "    jmp nova_user_return",
    ".size Xfast_syscall, . - Xfast_syscall",
    pc_kern_rsp = const PC_KERN_RSP,
    pc_user_regs = const PC_USER_REGS,
    pc_scratch = const PC_SCRATCH,
    pc_tss_rsp0 = const PC_TSS_RSP0,
    udata = const gsel(GUDATA_SEL, SEL_UPL),
    ucode = const gsel(GUCODE_SEL, SEL_UPL),
    exit_syscall = const EXIT_SYSCALL,
    o_rax = const off!(rax),
    o_rbx = const off!(rbx),
    o_rcx = const off!(rcx),
    o_rdx = const off!(rdx),
    o_rsi = const off!(rsi),
    o_rdi = const off!(rdi),
    o_rbp = const off!(rbp),
    o_r8 = const off!(r8),
    o_r9 = const off!(r9),
    o_r10 = const off!(r10),
    o_r11 = const off!(r11),
    o_r12 = const off!(r12),
    o_r13 = const off!(r13),
    o_r14 = const off!(r14),
    o_r15 = const off!(r15),
    o_rip = const off!(rip),
    o_rflags = const off!(rflags),
    o_rsp = const off!(rsp),
);

unsafe extern "C" {
    fn nova_user_enter(regs: *mut UserRegs) -> u64;
    fn nova_user_return(kern_rsp: u64, code: u64) -> !;
    fn Xfast_syscall();
}

/// Sets up `syscall` on this CPU (FreeBSD's `amd64_conf_fast_syscall`):
/// kernel selectors on entry, the entry point, and the flags to clear.
pub(crate) fn syscall_init() {
    let star = (u64::from(gsel(GUCODE32_SEL, SEL_UPL)) << 48) | (u64::from(gsel(GCODE_SEL, SEL_KPL)) << 32);
    let entry: unsafe extern "C" fn() = Xfast_syscall;
    // SAFETY: these MSRs exist on every amd64 CPU. The selectors are the
    // GDT's (FreeBSD's layout, which `sysret`'s ordering requires); the
    // entry stub is `Xfast_syscall`; interrupts, tracing, direction and
    // alignment checks are cleared on entry, as in FreeBSD.
    unsafe {
        wrmsr(MSR_STAR, star);
        wrmsr(MSR_LSTAR, entry as usize as u64);
        wrmsr(MSR_SF_MASK, x86::psl::PSL_I | x86::psl::PSL_T | x86::psl::PSL_D | x86::psl::PSL_AC | x86::psl::PSL_NT);
        wrmsr(MSR_EFER, rdmsr(MSR_EFER) | EFER_SCE);
    }
}

/// True for an address user mode may use as an instruction or stack
/// pointer: in the lower, user half.
fn user_address(a: u64) -> bool {
    a < VM_MAXUSER_ADDRESS
}

/// `mdi::UserMode::cpu_user_enter`.
///
/// # Safety
///
/// See `mdi::UserMode::cpu_user_enter`.
pub(crate) unsafe fn cpu_user_enter(regs: &mut UserRegs) -> UserExit {
    // `iretq` with a kernel or non-canonical %rip/%rsp would fault in the
    // kernel; refuse instead, as if the program had faulted.
    if !user_address(regs.rip) || !user_address(regs.rsp) {
        let addr = if user_address(regs.rip) { regs.rsp } else { regs.rip };
        return UserExit::Fault(UserFault { kind: FaultKind::Protection { write: false }, addr });
    }
    regs.rflags = user_rflags(regs.rflags);
    // SAFETY: interrupts are off (caller's contract), the current address
    // space is the program's, and the registers are sanitized above: user
    // %cs/%ss, IOPL 0, interrupts on, user-half %rip/%rsp. User mode can
    // only come back through `Xfast_syscall` or `user_trap`, which both end
    // in `nova_user_return` with this stack's saved context.
    let code = unsafe { nova_user_enter(regs) };
    match code {
        EXIT_SYSCALL => UserExit::Syscall,
        EXIT_INTERRUPT => UserExit::Interrupt,
        _ => UserExit::Fault(classify(regs)),
    }
}

/// A user fault in MI terms (what FreeBSD's `trap()` maps to signals).
fn classify(regs: &UserRegs) -> UserFault {
    const PGEX_P: u64 = 0x01;
    const PGEX_W: u64 = 0x02;
    let kind = match regs.trapno {
        12 => {
            let write = regs.err & PGEX_W != 0;
            if regs.err & PGEX_P != 0 { FaultKind::Protection { write } } else { FaultKind::NotMapped { write } }
        }
        9 | 25 | 26 | 27 => FaultKind::Protection { write: false }, // GP, TSS, NP, SS
        1 => FaultKind::IllegalInstruction,                          // T_PRIVINFLT (#UD)
        6 | 18 | 20 | 21 | 29 => FaultKind::Arithmetic,              // FPU, divide, overflow, bound, SIMD
        3 | 10 => FaultKind::Breakpoint,                             // T_BPTFLT, T_TRCTRAP
        other => FaultKind::Other(other),
    };
    UserFault { kind, addr: regs.fault_addr }
}

/// Called by `alltraps` for traps and interrupts taken in user mode, on
/// the thread's kernel stack below its saved context, with `%gs` already
/// swapped to the kernel's.
pub(crate) extern "C" fn user_trap(frame: &mut TrapFrame) -> ! {
    // Read the per-CPU state now: handling an interrupt may switch threads
    // and resume this one on another CPU.
    let pc = pcpu_self();
    let kern_rsp = pc.kern_rsp.load(core::sync::atomic::Ordering::Relaxed);
    let regs = pc.user_regs.load(core::sync::atomic::Ordering::Relaxed) as *mut UserRegs;
    let saved = UserRegs {
        rax: frame.tf_rax,
        rbx: frame.tf_rbx,
        rcx: frame.tf_rcx,
        rdx: frame.tf_rdx,
        rsi: frame.tf_rsi,
        rdi: frame.tf_rdi,
        rbp: frame.tf_rbp,
        r8: frame.tf_r8,
        r9: frame.tf_r9,
        r10: frame.tf_r10,
        r11: frame.tf_r11,
        r12: frame.tf_r12,
        r13: frame.tf_r13,
        r14: frame.tf_r14,
        r15: frame.tf_r15,
        rip: frame.tf_rip,
        rflags: frame.tf_rflags,
        rsp: frame.tf_rsp,
        trapno: frame.tf_trapno,
        err: frame.tf_err,
        fault_addr: if frame.tf_trapno == 12 { rcr2() } else { 0 },
    };
    // SAFETY: `regs` is the `&mut UserRegs` that `cpu_user_enter` passed to
    // `nova_user_enter` on this thread, still borrowed by it (its frame is
    // above `kern_rsp`, untouched); nothing else accesses it meanwhile.
    unsafe { regs.write(saved) };
    let code = if frame.tf_trapno & T_INTR != 0 {
        // Acknowledge and dispatch as any interrupt; this may preempt.
        trap(frame);
        EXIT_INTERRUPT
    } else {
        EXIT_FAULT
    };
    // SAFETY: `kern_rsp` is this thread's context saved by
    // `nova_user_enter`, which is waiting to return.
    unsafe { nova_user_return(kern_rsp, code) }
}
