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

//! Thread context switch (FreeBSD's `amd64/cpu_switch.S`) and the first
//! frame of a new thread (`cpu_fork`'s `fork_trampoline`).
//!
//! A switch only has to preserve what the ABI says survives a call: the
//! callee-saved registers and the stack pointer. `cpu_switch` pushes them
//! on the old thread's stack, saves `%rsp` in its [`Pcb`], loads the new
//! thread's `%rsp` and pops its registers. The kernel uses no SSE/x87
//! (soft-float target), so there is no FPU state to switch.
//!
//! TODO(user): FPU/XSAVE state, `%fs`/`%gs` bases and the TSS `rsp0` once
//! threads run user code.

use core::arch::global_asm;

/// Saved context of a thread that is not running (FreeBSD's `struct pcb`,
/// reduced to what `cpu_switch` needs).
#[repr(C)]
pub struct Pcb {
    rsp: u64,
}

impl Pcb {
    /// A PCB that has never run.
    pub const INIT: Pcb = Pcb { rsp: 0 };
}

global_asm!(
    ".section .text",
    ".globl nova_cpu_switch",
    "nova_cpu_switch:", // rdi = old pcb, rsi = new pcb
    "    push rbp",
    "    push rbx",
    "    push r12",
    "    push r13",
    "    push r14",
    "    push r15",
    "    mov [rdi], rsp",
    "    mov rsp, [rsi]",
    "    pop r15",
    "    pop r14",
    "    pop r13",
    "    pop r12",
    "    pop rbx",
    "    pop rbp",
    "    ret",
    "",
    // A new thread "returns" here from its first cpu_switch, with the
    // entry point in %r12 and %rbp = 0 to end backtraces.
    ".globl fork_trampoline",
    "fork_trampoline:",
    "    call r12",
    "    ud2",
);

unsafe extern "C" {
    fn nova_cpu_switch(old: *mut Pcb, new: *const Pcb);
    fn fork_trampoline();
}

/// Words `nova_cpu_switch` pops: r15, r14, r13, r12, rbx, rbp, return address.
const SWITCH_FRAME_WORDS: usize = 7;
/// Index of `%r12` in that frame.
const FRAME_R12: usize = 3;

/// `mdi::Context::cpu_thread_init`.
///
/// # Safety
///
/// See `mdi::Context::cpu_thread_init`.
pub(crate) unsafe fn cpu_thread_init(pcb: &mut Pcb, stack_top: usize, entry: extern "C" fn() -> !) {
    assert!(
        stack_top.is_multiple_of(16),
        "cpu_thread_init: misaligned stack"
    );
    // After the six pops and `ret`, %rsp is `stack_top`, 16-byte aligned,
    // so `call r12` in the trampoline enters `entry` with the alignment the
    // ABI expects.
    let frame = (stack_top - SWITCH_FRAME_WORDS * 8) as *mut u64;
    // SAFETY: the caller guarantees the stack is mapped and writable; the
    // frame is the top 56 bytes of it.
    unsafe {
        for i in 0..SWITCH_FRAME_WORDS {
            frame.add(i).write(0);
        }
        frame.add(FRAME_R12).write(entry as usize as u64);
        let trampoline: unsafe extern "C" fn() = fork_trampoline;
        frame
            .add(SWITCH_FRAME_WORDS - 1)
            .write(trampoline as usize as u64);
    }
    pcb.rsp = frame as u64;
}

/// `mdi::Context::cpu_switch`.
///
/// # Safety
///
/// See `mdi::Context::cpu_switch`.
pub(crate) unsafe fn cpu_switch(old: *mut Pcb, new: *const Pcb) {
    // SAFETY: forwarded to the caller.
    unsafe { nova_cpu_switch(old, new) }
}
