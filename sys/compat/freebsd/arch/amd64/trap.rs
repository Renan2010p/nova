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

//! FreeBSD/amd64 system call arguments (`cpu_fetch_syscall_args`,
//! `amd64/amd64/trap.c`): the number in `%rax`, the arguments in `%rdi`,
//! `%rsi`, `%rdx`, `%r10`, `%r8`, `%r9`, as `syscall` clobbers `%rcx`.

use framework::user::UserRegs;

use crate::freebsd_syscalls::{SYS___syscall, SYS_syscall};

/// The system call number and its six register arguments. `syscall(2)`
/// and `__syscall(2)` take the real number as their first argument.
pub(crate) fn cpu_fetch_syscall_args(regs: &UserRegs) -> (u64, [u64; 6]) {
    let args = [regs.rdi, regs.rsi, regs.rdx, regs.r10, regs.r8, regs.r9];
    let code = regs.rax;
    if code == u64::from(SYS_syscall) || code == u64::from(SYS___syscall) {
        (args[0], [args[1], args[2], args[3], args[4], args[5], 0])
    } else {
        (code, args)
    }
}
