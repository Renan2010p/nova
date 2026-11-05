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


//! Process exit (FreeBSD's `kern/kern_exit.c`); waiting for children is
//! [`crate::kern_proc::kern_wait`].

use framework::user;
use sys::signal::Signal;

use crate::kern_proc::{PID_INIT, Proc, proc_zombie};

/// How a process ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExitStatus {
    /// It called `exit` with this status.
    Exited(i32),
    /// It was killed by this signal.
    Signaled(Signal),
}

/// Ends process `p` (`exit1`): its files, directory and memory go, it
/// becomes a zombie its parent reaps, and its children go to init. Called
/// on `p`'s thread, which then exits.
///
/// # Panics
///
/// If `p` is init, as in FreeBSD: the system cannot run without it.
pub fn exit1(p: &Proc, status: ExitStatus) {
    if p.pid == PID_INIT {
        let (signo, rval) = match status {
            ExitStatus::Exited(rv) => (0, rv),
            ExitStatus::Signaled(sig) => (p.abi().signals().map_or(0, |s| s.signal_number(sig)), 0),
        };
        crate::printf!("init died (signal {}, exit {})\n", signo, rval);
        panic!("Going nowhere without my init!");
    }
    *p.itimer.lock() = crate::kern_resource::Itimer::default();
    p.fd.close_all();
    *p.cwd.lock() = None;
    // A session leader's end leaves its terminal free (`killjobc`).
    if p.sid() == p.pid {
        crate::tty::tty_release(p.sid());
    }
    // The memory goes now, not when the zombie is reaped: an empty space
    // takes its place (`vmspace_exit`).
    user::set_vmspace(None);
    if let Ok(empty) = user::VmSpace::new() {
        drop(p.set_vm(empty));
    }
    proc_zombie(p, status);
    // A process with a descriptor tells through it, not with SIGCHLD.
    let pd = p.procdesc.load(core::sync::atomic::Ordering::Acquire);
    if let (false, Some(parent)) = (pd, crate::kern_proc::pfind(p.ppid())) {
        crate::kern_sig::psignal(&parent, Signal::SIGCHLD);
    }
}
