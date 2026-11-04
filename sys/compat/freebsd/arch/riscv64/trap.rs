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

//! FreeBSD/riscv system call arguments (`cpu_fetch_syscall_args`,
//! `riscv/riscv/trap.c`): the number in `t0`, the arguments in `a0`-`a7`.

use framework::user::{UserFault, UserRegs};
use kern::kern_proc::Proc;

use super::exec_machdep::A0;
use crate::freebsd_syscalls::{SYS___syscall, SYS_syscall};

/// `x` index of `t0` (`x5`).
pub(crate) const T0: usize = 4;

/// The system call number and its first six arguments.
pub(crate) fn cpu_fetch_syscall_args(regs: &UserRegs) -> (u64, [u64; 6]) {
    let a = |i: usize| regs.x[A0 + i];
    let code = regs.x[T0];
    if code == u64::from(SYS_syscall) || code == u64::from(SYS___syscall) {
        (a(0), [a(1), a(2), a(3), a(4), a(5), a(6)])
    } else {
        (code, [a(0), a(1), a(2), a(3), a(4), a(5)])
    }
}

/// FreeBSD's `trap_uprintf_signal`: not written for riscv yet.
pub(crate) fn trap_uprintf_signal(_p: &Proc, _regs: &UserRegs, _fault: &UserFault, _signo: i32) {}
