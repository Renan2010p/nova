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

//! The FreeBSD system call table (`sysent[]`, `kern/init_sysent.c`):
//! decodes each call's arguments from the FreeBSD ABI and calls the
//! kernel's implementation. Numbers and names come from the generated
//! [`crate::freebsd_syscalls`].
//!
//! A number FreeBSD does not have is `nosys`, as in FreeBSD: `SIGSYS`. A
//! call FreeBSD has but Nova does not implement yet fails with `ENOSYS`
//! (a FreeBSD program may handle that), logged on a verbose boot.

use kern::kern_proc::{PID_INIT, Proc};
use kern::{printf, sys_generic};
use sys::errno::Errno;
use sys::signal::Signal;

use crate::freebsd_syscalls::{SYS__exit, SYS_MAXSYSCALL, SYS_write, SYSCALLNAMES};

/// The outcome of one system call.
pub(crate) enum SyscallResult {
    /// Success, with `td_retval[0..2]`.
    Ok([u64; 2]),
    /// Failure.
    Err(Errno),
    /// `_exit(rval)`.
    Exit(i32),
    /// A signal to the caller.
    Signal(Signal),
}

/// Runs system call `code` with `args` for `p`.
#[allow(non_upper_case_globals)] // FreeBSD's `SYS_*` names
pub(crate) fn syscall(p: &Proc, code: u64, args: &[u64; 6]) -> SyscallResult {
    let Ok(code) = u32::try_from(code) else {
        return nosys(p, code);
    };
    match code {
        SYS__exit => SyscallResult::Exit(args[0] as i32),
        SYS_write => match sys_generic::kern_write(p, args[0] as i32, args[1], args[2]) {
            Ok(n) => SyscallResult::Ok([n, 0]),
            Err(e) => SyscallResult::Err(e),
        },
        c if c >= SYS_MAXSYSCALL || is_nosys(SYSCALLNAMES[c as usize]) => nosys(p, u64::from(c)),
        c => {
            if kern::init_main::bootverbose() {
                printf!(
                    "pid {} comm {}: {} ({}) not implemented\n",
                    p.pid,
                    p.comm,
                    SYSCALLNAMES[c as usize],
                    c
                );
            }
            SyscallResult::Err(Errno::ENOSYS)
        }
    }
}

/// Slots without a system call: reserved, obsolete, or for loadable ones.
fn is_nosys(name: &str) -> bool {
    name.starts_with('#') || name.starts_with("obs_") || name == "lkmnosys"
}

/// `kern_nosys`: `SIGSYS` (the default `kern.signosys`), logged for init
/// (the default `kern.lognosys`).
fn nosys(p: &Proc, code: u64) -> SyscallResult {
    if p.pid == PID_INIT {
        printf!("pid {} comm {}: nosys {}\n", p.pid, p.comm, code);
    }
    SyscallResult::Signal(Signal::SIGSYS)
}
