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

//! The kernel's own error codes.
//!
//! They carry POSIX names, because that is what they mean, but they have
//! no numeric values a program could see: every ABI in `sys/compat`
//! translates them to its own numbers (FreeBSD's `EBADF` is 9, another
//! system's may differ). The kernel never returns a number to userland.
//!
//! The enum is deliberately exhaustive: an ABI's translation is a `match`
//! without a wildcard, so adding an error here fails to compile until
//! every ABI has a number for it.

/// An error from a kernel operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Errno {
    /// Operation not permitted.
    EPERM,
    /// No such file or directory.
    ENOENT,
    /// No such process.
    ESRCH,
    /// Interrupted system call.
    EINTR,
    /// Input/output error.
    EIO,
    /// Argument list too long.
    E2BIG,
    /// Exec format error.
    ENOEXEC,
    /// Bad file descriptor.
    EBADF,
    /// Cannot allocate memory.
    ENOMEM,
    /// Permission denied.
    EACCES,
    /// Bad address.
    EFAULT,
    /// Invalid argument.
    EINVAL,
    /// Inappropriate ioctl for device.
    ENOTTY,
    /// Result too large.
    ERANGE,
    /// Resource temporarily unavailable.
    EAGAIN,
    /// Function not implemented.
    ENOSYS,
}

/// The result of a kernel operation.
pub type KResult<T> = Result<T, Errno>;
