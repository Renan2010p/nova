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

use crate::kern_exit::{ExitStatus, exit1};
use crate::kern_proc::Proc;
use crate::kern_sig::sigexit;
use crate::kern_abi::SyscallOutcome;

/// Runs `p` from `regs` until it exits. Called on `p`'s thread.
pub fn userret_loop(p: &Proc, mut regs: UserRegs) -> ExitStatus {
    user::set_vmspace(Some(p.vm.clone()));
    let status = loop {
        match user::enter(&mut regs) {
            UserExit::Syscall => match p.abi.syscall(p, &mut regs) {
                SyscallOutcome::Return => {}
                SyscallOutcome::Exit(rv) => break ExitStatus::Exited(rv),
                SyscallOutcome::Signal(sig) => break sigexit(p, sig),
            },
            UserExit::Interrupt => {}
            UserExit::Fault(fault) => break sigexit(p, p.abi.trapsignal(&fault)),
        }
    };
    exit1(p, status);
    user::set_vmspace(None);
    status
}
