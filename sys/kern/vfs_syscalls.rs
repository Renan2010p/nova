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

//! File system system calls (FreeBSD's `kern/vfs_syscalls.c`).
//!
//! TODO(vfs): there is no file system yet, so every path names nothing.

use sys::errno::{Errno, KResult};

use crate::kern_proc::Proc;

/// Reads a symbolic link (`kern_readlinkat`). No path exists yet.
pub fn kern_readlinkat(_p: &Proc, _path: u64, _buf: u64, _len: u64) -> KResult<u64> {
    Err(Errno::ENOENT)
}
