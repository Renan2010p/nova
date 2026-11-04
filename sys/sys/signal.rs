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
//! Signals, by name.
//!
//! As with [`crate::errno`], the kernel knows what a signal means but not
//! its number: every ABI in `sys/compat` maps them to its own (`SIGBUS`
//! is 10 for FreeBSD, 7 for some other systems). Exhaustive, for the same
//! reason as `Errno`.

/// A signal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Signal {
    /// Illegal instruction.
    SIGILL,
    /// Trace or breakpoint trap.
    SIGTRAP,
    /// Arithmetic exception.
    SIGFPE,
    /// Kill (cannot be caught or ignored).
    SIGKILL,
    /// Bus error.
    SIGBUS,
    /// Segmentation violation.
    SIGSEGV,
    /// Bad system call.
    SIGSYS,
}

impl Signal {
    /// True if the default action dumps core (`SIGPROP_CORE`).
    pub const fn dumps_core(self) -> bool {
        !matches!(self, Signal::SIGKILL)
    }
}
