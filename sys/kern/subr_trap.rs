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

//! Running a process (FreeBSD's `kern/subr_trap.c`, `userret`, and the MI
//! half of `syscall`/`trap`).
//!
//! The loop is the whole life of a user thread: run it in user mode, and
//! when it comes back, let the ABI handle a system call, or raise the
//! signal a fault means.

use framework::user::{self, UserExit, UserRegs};
use sys::signal::Signal;

use crate::kern_exit::{ExitStatus, exit1};
use crate::kern_proc::Proc;
use crate::kern_sig::{postsig, sigexit};
use crate::kern_abi::SyscallOutcome;

/// Runs `p` from `regs` until it exits. Called on `p`'s thread.
pub fn userret_loop(p: &Proc, mut regs: UserRegs) -> ExitStatus {
    user::set_vmspace(Some(p.vm()));
    let status = loop {
        // Signals first (`ast`): a handler's frame, a stop, or the end.
        if p.sig_pending()
            && let Some(status) = postsig(p, &mut regs)
        {
            break status;
        }
        let exit = user::enter(&mut regs);
        match exit {
            UserExit::Syscall => match p.abi().syscall(p, &mut regs) {
                SyscallOutcome::Return => {}
                SyscallOutcome::Exit(rv) => break ExitStatus::Exited(rv),
                SyscallOutcome::Signal(sig) => break sigexit(p, sig),
            },
            UserExit::Interrupt => {}
            UserExit::Fault(fault) => {
                // Memory mapped but not present yet is paged in.
                if vm::vm_fault::vm_fault_trap(&p.vm(), &fault) {
                    continue;
                }
                // An ABI with no signal model has no fault signal to
                // raise: the process is terminated.
                let sig = p.abi().signals().map_or(Signal::SIGKILL, |s| s.trapsignal(p, &fault, &regs));
                break sigexit(p, sig);
            }
        }
    };
    exit1(p, status);
    status
}
