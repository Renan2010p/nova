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

use framework::user::{UserFault, UserRegs};
use kern::kern_proc::Proc;
use kern::printf;

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

/// FreeBSD's `trap_uprintf_signal` (with `machdep.uprintf_signal=1`): what
/// the program was doing when a fault killed it, in FreeBSD's format. Nova
/// prints it on a verbose boot.
pub(crate) fn trap_uprintf_signal(p: &Proc, regs: &UserRegs, fault: &UserFault, signo: i32) {
    let mut code = [0u8; 8];
    let shown = p.vm().copyin(regs.rip, &mut code).is_ok();
    printf!(
        "pid {} comm {}: signal {} err {:#x} code {} type {} addr {:#x} rsp {:#x} rip {:#x} rax {:#x} fsb {:#x} gsb {:#x} ",
        p.pid,
        p.comm(),
        signo,
        regs.err,
        0,
        regs.trapno,
        fault.addr,
        regs.rsp,
        regs.rip,
        regs.rax,
        regs.fsbase,
        0
    );
    if shown {
        printf!(
            "<{:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x}>\n",
            code[0], code[1], code[2], code[3], code[4], code[5], code[6], code[7]
        );
    } else {
        printf!("<?>\n");
    }
}
