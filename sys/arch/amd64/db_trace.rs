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

//! Stack traces (FreeBSD's `amd64/db_trace.c`: `db_backtrace`,
//! `db_nextframe`).
//!
//! The kernel is built with frame pointers, so every frame starts with
//! the caller's `%rbp` followed by the return address. When a return
//! address lies in `calltrap`, the frame above `trap`'s is a trap frame,
//! as FreeBSD's `db_nextframe` knows: the walk reports it and goes on in
//! the interrupted code, on whatever stack that was (a thread's stack
//! below an IST stack, for a double fault).
//!
//! Every word is checked to be mapped kernel memory before it is read,
//! so a corrupt chain ends the trace instead of faulting inside a panic.

use libkern::conv::u64_from_usize;
use core::mem::{offset_of, size_of};

use mdi::StackFrame;

use crate::exception::calltrap_ret;
use crate::pmap::pmap_kextract;
use crate::trap::{T_INTR, TrapFrame};

/// Deepest trace walked.
const MAX_FRAMES: usize = 32;

/// Lowest kernel address on amd64.
const VM_MIN_KERNEL: usize = 0xffff_8000_0000_0000;

fn readable(va: usize) -> bool {
    va >= VM_MIN_KERNEL && va.is_multiple_of(8) && pmap_kextract(va).is_some()
}

/// Reads a word that `readable` accepted.
fn word(va: usize) -> usize {
    // SAFETY: callers only pass addresses `readable` accepted: aligned,
    // mapped kernel memory, read as a plain integer.
    unsafe { core::ptr::with_exposed_provenance::<usize>(va).read() }
}

/// `mdi::Cpu::stack_trace`.
#[inline(never)]
pub(crate) fn stack_trace(f: &mut dyn FnMut(StackFrame) -> bool) {
    let calltrap_ret: unsafe extern "C" fn() = calltrap_ret;
    let calltrap_ret = libkern::fn_addr!(calltrap_ret);
    let mut rbp: usize;
    // SAFETY: reads a register.
    unsafe {
        core::arch::asm!("mov {}, rbp", out(reg) rbp, options(nomem, nostack, preserves_flags))
    };
    // Frames of one stack lie at increasing addresses; a trap frame starts
    // a new stack, so the check restarts there.
    let mut floor = 0;
    for _ in 0..MAX_FRAMES {
        if rbp == 0 || rbp <= floor || !readable(rbp) || !readable(rbp + 8) {
            break;
        }
        let (next, ret) = (word(rbp), word(rbp + 8));
        if ret == 0 || !f(StackFrame::Return(ret)) {
            break;
        }
        floor = rbp;
        rbp = next;
        if ret != calltrap_ret {
            continue;
        }
        // `trap`'s frame is just below the trap frame (`*fp + 16`).
        let tf = floor + 16;
        let field = |off: usize| tf + off;
        if !(readable(tf) && readable(tf + size_of::<TrapFrame>() - 8)) {
            break;
        }
        let trapno = u64_from_usize(word(field(offset_of!(TrapFrame, tf_trapno))));
        let pc = word(field(offset_of!(TrapFrame, tf_rip)));
        let sp = word(field(offset_of!(TrapFrame, tf_rsp)));
        let fp = word(field(offset_of!(TrapFrame, tf_rbp)));
        let trap = StackFrame::Trap {
            interrupt: trapno & T_INTR != 0,
            trapno,
            pc,
            sp,
            fp,
        };
        if !f(trap) || !f(StackFrame::Pc(pc)) {
            break;
        }
        rbp = fp;
        floor = 0;
    }
}
