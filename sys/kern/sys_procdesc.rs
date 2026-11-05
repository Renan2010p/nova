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


//! Process descriptors (FreeBSD's `kern/sys_procdesc.c`): a process
//! created by `pdfork` is reached through a descriptor instead of its
//! parent's `wait`. Closing the last descriptor kills it (unless
//! `PD_DAEMON`) and reaps it; `poll` sees it end.

use core::sync::atomic::{AtomicBool, Ordering};

use framework::mm::KArc;

use crate::kern_proc::{Proc, pfind};

/// A process descriptor (`struct procdesc`).
#[derive(Debug)]
pub struct ProcDesc {
    /// The process.
    pub pid: u32,
    /// Let it live when the descriptor closes (`PD_DAEMON`).
    pub daemon: bool,
    closed: AtomicBool,
}

impl ProcDesc {
    /// A descriptor for `pid`.
    pub fn new(pid: u32, daemon: bool) -> Self {
        Self { pid, daemon, closed: AtomicBool::new(false) }
    }

    /// The process, while it is not reaped.
    pub fn proc(&self) -> Option<KArc<Proc>> {
        pfind(self.pid)
    }

    /// It ended (`POLLHUP`).
    pub fn exited(&self) -> bool {
        self.proc().is_none_or(|q| q.exit_status().is_some())
    }

    /// The last descriptor is gone (`procdesc_close`): the process is
    /// killed unless a daemon, and reaped when it ends.
    pub(crate) fn close(&self) {
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        let Some(q) = self.proc() else { return };
        q.set_autoreap();
        if !self.daemon && q.exit_status().is_none() {
            crate::kern_sig::psignal(&q, sys::signal::Signal::SIGKILL);
        }
    }
}
