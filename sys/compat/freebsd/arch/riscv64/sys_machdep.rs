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

//! FreeBSD/riscv `sysarch(2)`: none of its operations exist yet.

use framework::user::UserRegs;
use kern::kern_proc::Proc;
use sys::errno::{Errno, KResult};

/// `sysarch(op, parms)`.
pub(crate) fn sysarch(_p: &Proc, _regs: &mut UserRegs, _op: i32, _parms: u64) -> KResult<[u64; 2]> {
    Err(Errno::EINVAL)
}
