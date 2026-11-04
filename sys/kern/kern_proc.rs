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

//! Processes (FreeBSD's `kern/kern_proc.c` and `struct proc`).
//!
//! For now a process is one thread running one image, with the console as
//! its standard descriptors.
//!
//! TODO(proc): process table, fork, wait, credentials, descriptor table.

use alloc::string::String;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicU32, Ordering};

use framework::user::VmSpace;

use crate::kern_abi::SysEntVec;

/// The pid of `init`.
pub const PID_INIT: u32 = 1;

static NEXT_PID: AtomicU32 = AtomicU32::new(PID_INIT);

/// A process.
pub struct Proc {
    /// Process id (`p_pid`).
    pub pid: u32,
    /// Command name (`p_comm`): the last component of the path executed.
    pub comm: String,
    /// User id (`cr_uid`).
    pub uid: u32,
    /// Jail id (`pr_id`): 0, the host.
    pub jid: u32,
    /// The address space (`p_vmspace`).
    pub vm: Arc<VmSpace>,
    /// The ABI the image runs under (`p_sysent`).
    pub abi: &'static dyn SysEntVec,
}

impl Proc {
    /// A new process for an image just executed, with the next pid.
    pub fn new(path: &str, vm: Arc<VmSpace>, abi: &'static dyn SysEntVec) -> Self {
        let comm = path.rsplit('/').next().unwrap_or(path);
        Self {
            pid: NEXT_PID.fetch_add(1, Ordering::Relaxed),
            comm: comm.chars().take(MAXCOMLEN).collect(),
            uid: 0,
            jid: 0,
            vm,
            abi,
        }
    }
}

/// Longest command name (`MAXCOMLEN`).
pub const MAXCOMLEN: usize = 19;
