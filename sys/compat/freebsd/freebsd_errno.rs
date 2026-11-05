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

//! FreeBSD's error numbers (`sys/sys/errno.h`), from the kernel's
//! [`Errno`]. The `match` has no wildcard: a new kernel error does not
//! compile until it has a FreeBSD number here.

use sys::errno::Errno;

/// The FreeBSD number of `e`.
pub(crate) const fn errno(e: Errno) -> i32 {
    match e {
        Errno::EPERM => 1,
        Errno::ENOENT => 2,
        Errno::ESRCH => 3,
        Errno::EINTR => 4,
        Errno::EIO => 5,
        Errno::E2BIG => 7,
        Errno::ENOEXEC => 8,
        Errno::EBADF => 9,
        Errno::ENOMEM => 12,
        Errno::EACCES => 13,
        Errno::EFAULT => 14,
        Errno::EINVAL => 22,
        Errno::ENOTTY => 25,
        Errno::ERANGE => 34,
        Errno::EAGAIN => 35,
        Errno::ENOSYS => 78,
        Errno::ECHILD => 10,
        Errno::EBUSY => 16,
        Errno::EEXIST => 17,
        Errno::ENOTDIR => 20,
        Errno::EISDIR => 21,
        Errno::EMFILE => 24,
        Errno::ESPIPE => 29,
        Errno::EROFS => 30,
        Errno::ELOOP => 62,
        Errno::ENAMETOOLONG => 63,
        Errno::EPIPE => 32,
        Errno::ENXIO => 6,
        Errno::ENODEV => 19,
        Errno::ETIMEDOUT => 60,
        Errno::ENOTEMPTY => 66,
        Errno::EXDEV => 18,
        Errno::ENOTSOCK => 38,
        Errno::EDESTADDRREQ => 39,
        Errno::EMSGSIZE => 40,
        Errno::EPROTOTYPE => 41,
        Errno::ENOPROTOOPT => 42,
        Errno::EPROTONOSUPPORT => 43,
        Errno::EOPNOTSUPP => 45,
        Errno::EAFNOSUPPORT => 47,
        Errno::EADDRINUSE => 48,
        Errno::ENOBUFS => 55,
        Errno::EISCONN => 56,
        Errno::ENOTCONN => 57,
        Errno::ECONNREFUSED => 61,
        // Never returned: the ABI restarts the call instead.
        Errno::ERESTART => -1,
    }
}
