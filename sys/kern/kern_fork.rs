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
    fork1(p, regs, false)
}

/// Forks `p` with a process descriptor for the child (`pdfork`): returns
/// the child's pid and the descriptor's number.
pub fn kern_pdfork(p: &Proc, regs: UserRegs, daemon: bool, cloexec: bool) -> KResult<(u32, i32)> {
    let pid = fork1(p, regs, true)?;
    let pd = crate::sys_procdesc::ProcDesc::new(pid, daemon);
    let pd = framework::mm::KArc::new(pd, &crate::kern_malloc::M_TEMP, framework::mm::M_WAITOK)?;
    let file = crate::kern_descrip::File::new(crate::kern_descrip::FileKind::ProcDesc(pd), crate::kern_descrip::FileFlags::default())?;
    let fd = p.fd.alloc(file, 0, cloexec)?;
    Ok((pid, fd))
}

fn fork1(p: &Proc, regs: UserRegs, procdesc: bool) -> KResult<u32> {
    let vm = p.vm().fork().map_err(vm_errno)?;
    let child = p.fork(vm)?;
    child.procdesc.store(procdesc, core::sync::atomic::Ordering::Release);
    let child = proc_register(child)?;
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
