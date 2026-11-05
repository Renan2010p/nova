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


//! Creating processes (FreeBSD's `kern/kern_fork.c`).
//!
//! The child gets a copy of the address space (`vmspace_fork`, honouring
//! `minherit`), the same open files, directory and signal actions, and a
//! thread of its own that starts from the registers the ABI prepared (the
//! parent's, with the child's return values).
//!
//! TODO(fork): copy-on-write, `rfork` flags, `vfork` sharing.

use framework::user::UserRegs;
use sys::errno::KResult;

use crate::kern_exec::vm_errno;
use crate::kern_proc::{Proc, proc_forget, proc_register};
use crate::{kern_kthread, subr_trap};

/// Forks `p` (`fork1`); the child starts in user mode with `regs`.
/// Returns the child's pid.
pub fn kern_fork(p: &Proc, regs: UserRegs) -> KResult<u32> {
    let vm = p.vm().fork().map_err(vm_errno)?;
    let child = proc_register(p.fork(vm)?)?;
    let pid = child.pid;
    let comm = child.comm();
    let me = child.clone();
    let thread = kern_kthread::kthread_add(comm.as_str(), move || {
        subr_trap::userret_loop(&child, regs);
    });
    match thread {
        Ok(t) => me.tid.store(t.tid(), core::sync::atomic::Ordering::Relaxed),
        Err(e) => {
            proc_forget(pid);
            return Err(e);
        }
    }
    Ok(pid)
}
