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

//! Generic I/O system calls (FreeBSD's `kern/sys_generic.c`), as the
//! kernel implements them; ABIs decode their arguments and call these.
//!
//! TODO(fd): a descriptor table. For now descriptors 0, 1 and 2 are the
//! console, as `init` gets them from `/dev/console`.

use libkern::conv::{u64_from_usize, usize_from_u64};
use sys::errno::{Errno, KResult};

use crate::kern_exec::vm_errno;
use crate::kern_proc::Proc;

/// Largest single write (`IOSIZE_MAX`).
const IOSIZE_MAX: u64 = i64::MAX.unsigned_abs();

/// Writes `nbyte` bytes from user address `buf` to descriptor `fd`
/// (`kern_writev`). Returns the bytes written.
pub fn kern_write(p: &Proc, fd: i32, buf: u64, nbyte: u64) -> KResult<u64> {
    if !(1..=2).contains(&fd) {
        return Err(Errno::EBADF);
    }
    if nbyte > IOSIZE_MAX {
        return Err(Errno::EINVAL);
    }
    let mut chunk = [0u8; 256];
    let mut done = 0;
    while done < nbyte {
        let n = usize_from_u64((nbyte - done).min(u64_from_usize(chunk.len())));
        if let Err(e) = p.vm.copyin(buf + done, &mut chunk[..n]) {
            // As FreeBSD: a partial transfer is a success.
            return if done > 0 { Ok(done) } else { Err(vm_errno(e)) };
        }
        crate::subr_prf::cnwrite(&chunk[..n]);
        done += u64_from_usize(n);
    }
    Ok(done)
}
