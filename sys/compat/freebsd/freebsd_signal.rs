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

//! FreeBSD's signal numbers (`sys/sys/signal.h`), from the kernel's
//! [`Signal`]. Exhaustive, as [`crate::freebsd_errno`].

use framework::user::{FaultKind, UserFault};
use sys::signal::Signal;

/// The FreeBSD number of `sig`.
pub(crate) const fn signo(sig: Signal) -> i32 {
    match sig {
        Signal::SIGILL => 4,
        Signal::SIGTRAP => 5,
        Signal::SIGFPE => 8,
        Signal::SIGKILL => 9,
        Signal::SIGBUS => 10,
        Signal::SIGSEGV => 11,
        Signal::SIGSYS => 12,
    }
}

/// The signal a user fault raises, as FreeBSD's `trap()` decides it: an
/// unmapped page is `SIGSEGV`, a protection fault (a page the program may
/// not access that way, or a general protection fault) `SIGBUS`.
pub(crate) fn trapsignal(fault: &UserFault) -> Signal {
    match fault.kind {
        FaultKind::NotMapped { .. } => Signal::SIGSEGV,
        FaultKind::Protection { .. } => Signal::SIGBUS,
        FaultKind::IllegalInstruction => Signal::SIGILL,
        FaultKind::Arithmetic => Signal::SIGFPE,
        FaultKind::Breakpoint => Signal::SIGTRAP,
        FaultKind::Other(_) => Signal::SIGBUS,
    }
}
