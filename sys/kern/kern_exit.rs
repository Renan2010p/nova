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

//! Process exit (FreeBSD's `kern/kern_exit.c`).
//!
//! TODO(proc): zombies, `wait`, reparenting to init.

use sys::signal::Signal;

use crate::kern_proc::{PID_INIT, Proc};

/// How a process ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExitStatus {
    /// It called `exit` with this status.
    Exited(i32),
    /// It was killed by this signal.
    Signaled(Signal),
}

/// Ends process `p` (`exit1`). The caller's thread then exits.
///
/// # Panics
///
/// If `p` is init, as in FreeBSD: the system cannot run without it.
pub fn exit1(p: &Proc, status: ExitStatus) {
    if p.pid == PID_INIT {
        let (signo, rval) = match status {
            ExitStatus::Exited(rv) => (0, rv),
            ExitStatus::Signaled(sig) => (p.abi.signal_number(sig), 0),
        };
        crate::printf!("init died (signal {}, exit {})\n", signo, rval);
        panic!("Going nowhere without my init!");
    }
}
