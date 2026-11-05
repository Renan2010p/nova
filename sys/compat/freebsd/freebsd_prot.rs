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


//! FreeBSD's credential calls: `setuid` and friends, `getresuid`,
//! `getgroups`/`setgroups` (FreeBSD 15's), with `-1` as "unchanged".

use kern::kern_exec::vm_errno;
use kern::kern_proc::{NGROUPS, Proc};
use kern::kern_prot::{
    kern_getcred, kern_setegid, kern_seteuid, kern_setgid, kern_setgroups, kern_setregid, kern_setresgid,
    kern_setresuid, kern_setreuid, kern_setuid,
};
use libkern::conv::{u64_from_usize, usize_from_u64};
use sys::errno::{Errno, KResult};

/// An id argument: `-1` (as `uid_t`) means "leave it".
fn id(v: u64) -> Option<u32> {
    u32::try_from(v & 0xffff_ffff).ok().filter(|&x| x != u32::MAX)
}

fn done(r: KResult<()>) -> KResult<[u64; 2]> {
    r.map(|()| [0, 0])
}

/// `setuid(uid)`.
pub(crate) fn sys_setuid(p: &Proc, uid: u64) -> KResult<[u64; 2]> {
    done(kern_setuid(p, id(uid).ok_or(Errno::EINVAL)?))
}

/// `seteuid(euid)`.
pub(crate) fn sys_seteuid(p: &Proc, uid: u64) -> KResult<[u64; 2]> {
    done(kern_seteuid(p, id(uid).ok_or(Errno::EINVAL)?))
}

/// `setgid(gid)`.
pub(crate) fn sys_setgid(p: &Proc, gid: u64) -> KResult<[u64; 2]> {
    done(kern_setgid(p, id(gid).ok_or(Errno::EINVAL)?))
}

/// `setegid(egid)`.
pub(crate) fn sys_setegid(p: &Proc, gid: u64) -> KResult<[u64; 2]> {
    done(kern_setegid(p, id(gid).ok_or(Errno::EINVAL)?))
}

/// `setreuid(ruid, euid)`.
pub(crate) fn sys_setreuid(p: &Proc, r: u64, e: u64) -> KResult<[u64; 2]> {
    done(kern_setreuid(p, id(r), id(e)))
}

/// `setregid(rgid, egid)`.
pub(crate) fn sys_setregid(p: &Proc, r: u64, e: u64) -> KResult<[u64; 2]> {
    done(kern_setregid(p, id(r), id(e)))
}

/// `setresuid(ruid, euid, suid)`.
pub(crate) fn sys_setresuid(p: &Proc, r: u64, e: u64, s: u64) -> KResult<[u64; 2]> {
    done(kern_setresuid(p, id(r), id(e), id(s)))
}

/// `setresgid(rgid, egid, sgid)`.
pub(crate) fn sys_setresgid(p: &Proc, r: u64, e: u64, s: u64) -> KResult<[u64; 2]> {
    done(kern_setresgid(p, id(r), id(e), id(s)))
}

/// `getresuid(ruid, euid, suid)` and `getresgid` (`group`).
pub(crate) fn sys_getres(p: &Proc, group: bool, ptrs: [u64; 3]) -> KResult<[u64; 2]> {
    let c = kern_getcred(p);
    let ids = if group { [c.rgid, c.egid, c.svgid] } else { [c.ruid, c.euid, c.svuid] };
    let vm = p.vm();
    for (ptr, v) in ptrs.into_iter().zip(ids) {
        if ptr != 0 {
            vm.copyout(ptr, &v.to_le_bytes()).map_err(vm_errno)?;
        }
    }
    Ok([0, 0])
}

/// `getgroups(gidsetsize, gidset)`: the count alone with a size of 0.
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_getgroups(p: &Proc, size: u64, set: u64) -> KResult<[u64; 2]> {
    let c = kern_getcred(p);
    let g = c.groups();
    if size == 0 {
        return Ok([u64_from_usize(g.len()), 0]);
    }
    if usize_from_u64(size) < g.len() {
        return Err(Errno::EINVAL);
    }
    let vm = p.vm();
    for (i, gid) in g.iter().enumerate() {
        vm.copyout(set + 4 * u64_from_usize(i), &gid.to_le_bytes()).map_err(vm_errno)?;
    }
    Ok([u64_from_usize(g.len()), 0])
}

/// `setgroups(ngroups, gidset)`.
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_setgroups(p: &Proc, n: u64, set: u64) -> KResult<[u64; 2]> {
    let n = usize_from_u64(n);
    if n > NGROUPS {
        return Err(Errno::EINVAL);
    }
    let mut g = [0u32; NGROUPS];
    let vm = p.vm();
    for (i, slot) in g.iter_mut().take(n).enumerate() {
        let mut b = [0u8; 4];
        vm.copyin(set + 4 * u64_from_usize(i), &mut b).map_err(vm_errno)?;
        *slot = u32::from_le_bytes(b);
    }
    done(kern_setgroups(p, g.get(..n).unwrap_or(&[])))
}
