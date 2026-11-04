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

//! FreeBSD/riscv system call results (`cpu_set_syscall_retval`,
//! `riscv/riscv/vm_machdep.c`): values in `a0` and `a1`; `t0` is 0 on
//! success, 1 with the error number in `a0`.

use framework::user::UserRegs;

use super::exec_machdep::A0;
use super::trap::T0;

/// Stores a result: `Ok([a0, a1])` or `Err(errno)`.
pub(crate) fn cpu_set_syscall_retval(regs: &mut UserRegs, result: Result<[u64; 2], i32>) {
    match result {
        Ok([v0, v1]) => {
            regs.x[A0] = v0;
            regs.x[A0 + 1] = v1;
            regs.x[T0] = 0;
        }
        Err(errno) => {
            regs.x[A0] = u64::from(errno.unsigned_abs());
            regs.x[T0] = 1;
        }
    }
}
