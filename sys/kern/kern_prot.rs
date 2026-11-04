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

//! Process identity and credentials (FreeBSD's `kern/kern_prot.c`).
//!
//! TODO(cred): users, groups, set-id programs.

use crate::kern_proc::Proc;

/// The process id (`getpid`).
pub fn kern_getpid(p: &Proc) -> u32 {
    p.pid
}

/// Whether the process is tainted by a set-id execution (`issetugid`).
/// Nothing runs set-id yet.
pub fn kern_issetugid(_p: &Proc) -> bool {
    false
}
