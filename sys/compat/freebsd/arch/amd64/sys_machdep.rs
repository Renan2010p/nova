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

//! FreeBSD/amd64 `sysarch(2)` (`amd64/amd64/sys_machdep.c`): the thread
//! pointer of TLS lives in `%fs`; FreeBSD 15's libc registers it with
//! `AMD64_SET_TLSBASE`.

use framework::user::{UserRegs, VM_MAXUSER_ADDRESS};
use kern::kern_exec::vm_errno;
use kern::kern_proc::Proc;
use sys::errno::{Errno, KResult};

const AMD64_GET_FSBASE: i32 = 128;
const AMD64_SET_FSBASE: i32 = 129;
const AMD64_GET_TLSBASE: i32 = 135;
const AMD64_SET_TLSBASE: i32 = 136;
const AMD64_DISABLE_TLSBASE: i32 = 137;

/// `sysarch(op, parms)`.
pub(crate) fn sysarch(p: &Proc, regs: &mut UserRegs, op: i32, parms: u64) -> KResult<[u64; 2]> {
    match op {
        AMD64_GET_FSBASE => {
            p.vm.copyout(parms, &regs.fsbase.to_le_bytes()).map_err(vm_errno)?;
            Ok([0, 0])
        }
        AMD64_SET_FSBASE | AMD64_SET_TLSBASE => {
            let mut b = [0u8; 8];
            p.vm.copyin(parms, &mut b).map_err(vm_errno)?;
            let base = u64::from_le_bytes(b);
            if base >= VM_MAXUSER_ADDRESS {
                return Err(Errno::EINVAL);
            }
            regs.fsbase = base;
            if op == AMD64_SET_TLSBASE {
                regs.tlsbase = base;
                regs.tlsbase_set = true;
            }
            Ok([0, 0])
        }
        AMD64_GET_TLSBASE => {
            if !regs.tlsbase_set {
                return Err(Errno::ESRCH);
            }
            p.vm.copyout(parms, &regs.tlsbase.to_le_bytes()).map_err(vm_errno)?;
            Ok([0, 0])
        }
        AMD64_DISABLE_TLSBASE => {
            regs.tlsbase_set = false;
            Ok([0, 0])
        }
        _ => Err(Errno::EINVAL),
    }
}
