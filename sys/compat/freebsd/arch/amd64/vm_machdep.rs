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

//! FreeBSD/amd64 system call results (`cpu_set_syscall_retval`,
//! `amd64/amd64/vm_machdep.c`): values in `%rax` and `%rdx`, and the
//! carry flag telling success (clear) from an error number in `%rax`.

use framework::user::UserRegs;

/// `PSL_C`, the carry flag.
const PSL_C: u64 = 0x1;

/// Stores a result: `Ok([rax, rdx])` or `Err(errno)`.
pub(crate) fn cpu_set_syscall_retval(regs: &mut UserRegs, result: Result<[u64; 2], i32>) {
    match result {
        Ok([v0, v1]) => {
            regs.rax = v0;
            regs.rdx = v1;
            regs.rflags &= !PSL_C;
        }
        Err(errno) => {
            regs.rax = errno as u64;
            regs.rflags |= PSL_C;
        }
    }
}
