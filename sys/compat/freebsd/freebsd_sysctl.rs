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

//! FreeBSD's `sysctl(3)` names (`kern/kern_sysctl.c`): the MIB a program
//! asks for, translated to what the kernel knows.
//!
//! TODO(sysctl): the tree. For now every name is unknown (`ENOENT`, as
//! FreeBSD answers for a name it lacks), and a verbose boot shows which
//! ones programs ask for.

use kern::kern_exec::vm_errno;
use kern::kern_proc::Proc;
use kern::printf;
use libkern::conv::{trunc_u32, usize_from_u64};
use sys::errno::{Errno, KResult};

/// `CTL_MAXNAME`: the longest MIB.
const CTL_MAXNAME: u64 = 24;

/// `__sysctl(name, namelen, old, oldlenp, new, newlen)`.
pub(crate) fn sys_sysctl(p: &Proc, args: &[u64; 6]) -> KResult<[u64; 2]> {
    let (name, namelen) = (args[0], u64::from(trunc_u32(args[1])));
    if !(2..=CTL_MAXNAME).contains(&namelen) {
        return Err(Errno::EINVAL);
    }
    let mut buf = [0u8; 4];
    let mut mib = [0i32; 24];
    for (i, slot) in mib.iter_mut().enumerate().take(usize_from_u64(namelen)) {
        p.vm.copyin(name + 4 * u64::try_from(i).map_err(|_| Errno::EINVAL)?, &mut buf).map_err(vm_errno)?;
        *slot = i32::from_le_bytes(buf);
    }
    if kern::init_main::bootverbose() {
        printf!("pid {} comm {}: sysctl", p.pid, p.comm());
        for x in mib.iter().take(usize_from_u64(namelen)) {
            printf!(" {}", x);
        }
        printf!(": unknown\n");
    }
    Err(Errno::ENOENT)
}
