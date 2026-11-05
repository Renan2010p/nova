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


//! FreeBSD/riscv signal frames: not yet (compile-only port).

use framework::user::{UserRegs, VmSpace};
use sys::errno::{Errno, KResult};
use sys::signal::{SigAction, SigSet};

/// Maps the signal trampoline: nothing yet.
pub(crate) fn map_sigcode(_vm: &VmSpace) -> KResult<()> {
    Ok(())
}

/// TODO(riscv): `sendsig`.
pub(crate) fn sendsig(
    _vm: &VmSpace,
    _signo: i32,
    _handler: u64,
    _action: &SigAction,
    _mask: SigSet,
    _regs: &mut UserRegs,
) -> KResult<()> {
    Err(Errno::ENOSYS)
}

/// TODO(riscv): `sys_sigreturn`.
pub(crate) fn sigreturn(_vm: &VmSpace, _ucp: u64, _regs: &mut UserRegs) -> KResult<SigSet> {
    Err(Errno::ENOSYS)
}

/// Back over the 4-byte `ecall`.
pub(crate) fn cpu_syscall_restart(regs: &mut UserRegs) {
    regs.sepc -= 4;
}

/// TODO(riscv): `sys_getcontext`.
pub(crate) fn getcontext(_vm: &VmSpace, _regs: &UserRegs, _mask: SigSet, _ucp: u64) -> KResult<()> {
    Err(Errno::ENOSYS)
}
