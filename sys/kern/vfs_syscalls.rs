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

/// The current directory's path (`kern___getcwd`): the root, the only
/// directory there is.
pub fn kern_getcwd(_p: &Proc) -> &'static str {
    "/"
}

/// The status of a path (`kern_statat`). No path exists yet; with
/// `empty_path` and an empty path, the status of the descriptor itself.
pub fn kern_statat(p: &Proc, fd: i32, path_empty: bool, empty_path: bool) -> KResult<crate::kern_descrip::Stat> {
    if path_empty && empty_path {
        return crate::sys_generic::kern_fstat(p, fd);
    }
    Err(Errno::ENOENT)
}
