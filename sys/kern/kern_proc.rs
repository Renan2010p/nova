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

use core::sync::atomic::{AtomicU32, Ordering};

use framework::mm::KArc;
use framework::sync::MtxSpin;
use framework::user::VmSpace;
use sys::signal::{NSIG, SigAction, SigSet};

use crate::kern_abi::SysEntVec;

/// The pid of `init`.
pub const PID_INIT: u32 = 1;

static NEXT_PID: AtomicU32 = AtomicU32::new(PID_INIT);

/// A process.
pub struct Proc {
    /// Process id (`p_pid`).
    pub pid: u32,
    /// Command name (`p_comm`): the last component of the path executed,
    /// at most [`MAXCOMLEN`] bytes, in a fixed buffer as in FreeBSD.
    comm: [u8; MAXCOMLEN],
    comm_len: usize,
    /// User id (`cr_uid`).
    pub uid: u32,
    /// Jail id (`pr_id`): 0, the host.
    pub jid: u32,
    /// The address space (`p_vmspace`).
    pub vm: KArc<VmSpace>,
    /// The ABI the image runs under (`p_sysent`).
    pub abi: &'static dyn SysEntVec,
    /// Signal mask and actions (`td_sigmask`, `p_sigacts`).
    pub sig: MtxSpin<SigState>,
}

/// A process's signal state. One thread per process for now, so the mask
/// is the process's.
#[derive(Clone, Copy, Debug)]
pub struct SigState {
    /// Blocked signals (`td_sigmask`).
    pub mask: SigSet,
    /// What each signal does, by `Signal::index` (`ps_sigact`).
    pub actions: [SigAction; NSIG],
}

impl SigState {
    /// Nothing blocked, every action the default (a new image).
    pub const fn new() -> Self {
        Self { mask: SigSet::EMPTY, actions: [SigAction::DEFAULT; NSIG] }
    }
}

impl Default for SigState {
    fn default() -> Self {
        Self::new()
    }
}

impl Proc {
    /// Command name (`p_comm`).
    pub fn comm(&self) -> &str {
        let bytes = self.comm.get(..self.comm_len).unwrap_or(&[]);
        core::str::from_utf8(bytes).unwrap_or("?")
    }

    /// A new process for an image just executed, with the next pid.
    pub fn new(path: &str, vm: KArc<VmSpace>, abi: &'static dyn SysEntVec) -> Self {
        let name = path.rsplit('/').next().unwrap_or(path);
        // Truncated at a character boundary, so `comm()` is valid UTF-8.
        let mut comm_len = name.len().min(MAXCOMLEN);
        while !name.is_char_boundary(comm_len) {
            comm_len -= 1;
        }
        let mut comm = [0u8; MAXCOMLEN];
        if let (Some(dst), Some(src)) = (comm.get_mut(..comm_len), name.as_bytes().get(..comm_len)) {
            dst.copy_from_slice(src);
        }
        Self {
            pid: NEXT_PID.fetch_add(1, Ordering::Relaxed),
            comm,
            comm_len,
            uid: 0,
            jid: 0,
            vm,
            abi,
            sig: MtxSpin::new(SigState::new()),
        }
    }
}

/// Longest command name (`MAXCOMLEN`).
pub const MAXCOMLEN: usize = framework::task::MAXCOMLEN;
