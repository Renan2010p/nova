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


//! Permission checks on files (FreeBSD's `vaccess`, `kern/vfs_subr.c`,
//! and the privilege checks of `vfs_syscalls.c`), by the classic Unix
//! rules: the owner's, the group's or others' bits, whichever class the
//! credentials fall in; root (effective uid 0) may do anything but run a
//! file no one may run.
//!
//! TODO(acl): ACLs, file flags (`schg`, `uappnd`), the `MAC` framework.

use sys::errno::{Errno, KResult};

use crate::kern_proc::Cred;
use crate::vnode::{VAttr, VType};

/// What is asked of a file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Want {
    /// Read it (list it, for a directory).
    pub read: bool,
    /// Write it (add or remove names, for a directory).
    pub write: bool,
    /// Run it (search it, for a directory).
    pub exec: bool,
}

impl Want {
    /// Search a directory.
    pub const SEARCH: Want = Want { read: false, write: false, exec: true };
    /// Add or remove a name in a directory.
    pub const MODIFY_DIR: Want = Want { read: false, write: true, exec: true };
}

/// Whether `cred` is in group `gid` (`groupmember`).
pub fn groupmember(gid: u32, cred: &Cred) -> bool {
    cred.egid == gid || cred.groups().contains(&gid)
}

/// Whether `cred` may do `want` to a file with attributes `a`
/// (`vaccess`): `EACCES` if not.
pub fn vaccess(a: &VAttr, cred: &Cred, want: Want) -> KResult<()> {
    if cred.euid == 0 {
        // Root runs only what someone may run.
        if want.exec && a.kind != VType::Dir && a.perm & 0o111 == 0 {
            return Err(Errno::EACCES);
        }
        return Ok(());
    }
    let bits = if cred.euid == a.uid {
        (a.perm >> 6) & 7
    } else if groupmember(a.gid, cred) {
        (a.perm >> 3) & 7
    } else {
        a.perm & 7
    };
    if (want.read && bits & 4 == 0) || (want.write && bits & 2 == 0) || (want.exec && bits & 1 == 0) {
        return Err(Errno::EACCES);
    }
    Ok(())
}

/// Whether `cred` may remove or rename the entry for `victim` in `dir`
/// (`EPERM` if not): writing the directory is not enough when it is
/// sticky (`S_ISVTX`, as `/tmp`); then only the file's owner, the
/// directory's or root may.
pub fn may_unlink(dir: &VAttr, victim: &VAttr, cred: &Cred) -> KResult<()> {
    vaccess(dir, cred, Want::MODIFY_DIR)?;
    let sticky = dir.perm & 0o1000 != 0;
    if sticky && cred.euid != 0 && cred.euid != victim.uid && cred.euid != dir.uid {
        return Err(Errno::EPERM);
    }
    Ok(())
}

/// Whether `cred` owns the file, or is root (`EPERM` if not): for
/// `chmod`, `chflags`, setting times explicitly.
pub fn owner_or_root(a: &VAttr, cred: &Cred) -> KResult<()> {
    if cred.euid == 0 || cred.euid == a.uid { Ok(()) } else { Err(Errno::EPERM) }
}

/// Whether `cred` may give the file owner `uid` and group `gid`
/// (`setfown`): only root changes the owner; the owner may change the
/// group to one of its own.
pub fn may_chown(a: &VAttr, cred: &Cred, uid: Option<u32>, gid: Option<u32>) -> KResult<()> {
    if cred.euid == 0 {
        return Ok(());
    }
    let uid_ok = uid.is_none_or(|u| u == a.uid);
    let gid_ok = gid.is_none_or(|g| g == a.gid || groupmember(g, cred));
    if cred.euid == a.uid && uid_ok && gid_ok { Ok(()) } else { Err(Errno::EPERM) }
}

/// Root only (`priv_check`): `EPERM` otherwise.
pub fn suser(cred: &Cred) -> KResult<()> {
    if cred.euid == 0 { Ok(()) } else { Err(Errno::EPERM) }
}
