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

//! Process identity and credentials (FreeBSD's `kern/kern_prot.c`).
//!
//! A process has real, effective and saved user and group ids, and
//! supplementary groups; root (effective uid 0) may change them freely,
//! others only among the ids they have (POSIX's rules, as FreeBSD's).
//!
//! TODO(cred): permission checks on files and processes for users other
//! than root (the kernel grants everything yet), set-id programs.

use sys::errno::{Errno, KResult};

use crate::kern_proc::{Cred, NGROUPS, Proc};

/// The process id (`getpid`).
pub fn kern_getpid(p: &Proc) -> u32 {
    p.pid
}

/// The parent's pid (`getppid`).
pub fn kern_getppid(p: &Proc) -> u32 {
    p.ppid()
}

/// Real and effective user and group ids (`getuid`, `geteuid`, `getgid`,
/// `getegid`).
pub fn kern_getcred(p: &Proc) -> Cred {
    p.cred()
}

fn root(c: &Cred) -> bool {
    c.euid == 0
}

/// Sets the user ids (`setuid`): all three as root, else the effective
/// one to the real or saved id.
pub fn kern_setuid(p: &Proc, uid: u32) -> KResult<()> {
    p.set_cred(|c| {
        if root(c) {
            c.ruid = uid;
            c.euid = uid;
            c.svuid = uid;
        } else if uid == c.ruid || uid == c.svuid {
            c.euid = uid;
        } else {
            return Err(Errno::EPERM);
        }
        Ok(())
    })
}

/// Sets the effective user id (`seteuid`).
pub fn kern_seteuid(p: &Proc, uid: u32) -> KResult<()> {
    p.set_cred(|c| {
        if !root(c) && uid != c.ruid && uid != c.svuid {
            return Err(Errno::EPERM);
        }
        c.euid = uid;
        Ok(())
    })
}

/// Sets the group ids (`setgid`), as `setuid` does the user ids.
pub fn kern_setgid(p: &Proc, gid: u32) -> KResult<()> {
    p.set_cred(|c| {
        if root(c) {
            c.rgid = gid;
            c.egid = gid;
            c.svgid = gid;
        } else if gid == c.rgid || gid == c.svgid {
            c.egid = gid;
        } else {
            return Err(Errno::EPERM);
        }
        Ok(())
    })
}

/// Sets the effective group id (`setegid`).
pub fn kern_setegid(p: &Proc, gid: u32) -> KResult<()> {
    p.set_cred(|c| {
        if !root(c) && gid != c.rgid && gid != c.svgid {
            return Err(Errno::EPERM);
        }
        c.egid = gid;
        Ok(())
    })
}

/// Sets real, effective and saved user ids (`setresuid`); `None` leaves
/// one; others than root may only use ids they have.
pub fn kern_setresuid(p: &Proc, r: Option<u32>, e: Option<u32>, s: Option<u32>) -> KResult<()> {
    p.set_cred(|c| {
        let ok = |id: Option<u32>| id.is_none_or(|i| i == c.ruid || i == c.euid || i == c.svuid);
        if !root(c) && !(ok(r) && ok(e) && ok(s)) {
            return Err(Errno::EPERM);
        }
        c.ruid = r.unwrap_or(c.ruid);
        c.euid = e.unwrap_or(c.euid);
        c.svuid = s.unwrap_or(c.svuid);
        Ok(())
    })
}

/// Sets real, effective and saved group ids (`setresgid`).
pub fn kern_setresgid(p: &Proc, r: Option<u32>, e: Option<u32>, s: Option<u32>) -> KResult<()> {
    p.set_cred(|c| {
        let ok = |id: Option<u32>| id.is_none_or(|i| i == c.rgid || i == c.egid || i == c.svgid);
        if !root(c) && !(ok(r) && ok(e) && ok(s)) {
            return Err(Errno::EPERM);
        }
        c.rgid = r.unwrap_or(c.rgid);
        c.egid = e.unwrap_or(c.egid);
        c.svgid = s.unwrap_or(c.svgid);
        Ok(())
    })
}

/// Sets real and effective user ids (`setreuid`): the saved id follows
/// the effective one when the real one is set or the effective one
/// differs from it.
pub fn kern_setreuid(p: &Proc, r: Option<u32>, e: Option<u32>) -> KResult<()> {
    p.set_cred(|c| {
        let real_ok = r.is_none_or(|i| i == c.ruid || i == c.euid);
        let eff_ok = e.is_none_or(|i| i == c.ruid || i == c.euid || i == c.svuid);
        if !root(c) && !(real_ok && eff_ok) {
            return Err(Errno::EPERM);
        }
        if let Some(e) = e {
            c.euid = e;
        }
        if r.is_some() || e.is_some_and(|e| e != c.ruid) {
            c.svuid = c.euid;
        }
        if let Some(r) = r {
            c.ruid = r;
        }
        Ok(())
    })
}

/// Sets real and effective group ids (`setregid`), as `setreuid`.
pub fn kern_setregid(p: &Proc, r: Option<u32>, e: Option<u32>) -> KResult<()> {
    p.set_cred(|c| {
        let real_ok = r.is_none_or(|i| i == c.rgid || i == c.egid);
        let eff_ok = e.is_none_or(|i| i == c.rgid || i == c.egid || i == c.svgid);
        if !root(c) && !(real_ok && eff_ok) {
            return Err(Errno::EPERM);
        }
        if let Some(e) = e {
            c.egid = e;
        }
        if r.is_some() || e.is_some_and(|e| e != c.rgid) {
            c.svgid = c.egid;
        }
        if let Some(r) = r {
            c.rgid = r;
        }
        Ok(())
    })
}

/// Sets the supplementary groups (`setgroups`): root only.
pub fn kern_setgroups(p: &Proc, groups: &[u32]) -> KResult<()> {
    if groups.len() > NGROUPS {
        return Err(Errno::EINVAL);
    }
    p.set_cred(|c| {
        if !root(c) {
            return Err(Errno::EPERM);
        }
        c.groups = [0; NGROUPS];
        if let Some(dst) = c.groups.get_mut(..groups.len()) {
            dst.copy_from_slice(groups);
        }
        c.ngroups = groups.len();
        Ok(())
    })
}

/// Whether the process is tainted by a set-id execution (`issetugid`).
/// Nothing runs set-id yet.
pub fn kern_issetugid(_p: &Proc) -> bool {
    false
}
