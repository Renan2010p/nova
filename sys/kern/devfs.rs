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


//! The device file system (FreeBSD's `fs/devfs`), mounted on `/dev` once
//! the root is: the kernel's devices as files. It is not a format on a
//! disk but the kernel's own names for its devices, so the kernel has it
//! (as Linux has `devtmpfs`).
//!
//! The devices: `console`, the console terminal (also under the names
//! the system policy gives it, FreeBSD's `ttyu0` and `ttyv0`); `tty`, the
//! caller's controlling terminal; `null` and `zero`.
//!
//! TODO(devfs): disks, `fd/` (`fdescfs`), `stdin`/`stdout`/`stderr`,
//! permissions and `devfs.rules`, devices that come and go.

use sys::errno::{Errno, KResult};

use crate::kern_descrip::FileKind;
use crate::kern_proc::Proc;
use crate::vnode::{DirEnt, StatFs, VAttr, VTime, VType, VfsOps};

/// A device (`struct cdev`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Cdev {
    Console,
    Tty,
    Null,
    Zero,
}

const ROOT: u64 = 2;

/// The device number of `/dev/console`.
pub const CONSOLE_DEV: u64 = 3;

/// The devices, by name and file number (also their device number).
const DEVICES: &[(&[u8], u64, Cdev, u16)] = &[
    (b"console", 3, Cdev::Console, 0o600),
    (b"null", 4, Cdev::Null, 0o666),
    (b"tty", 5, Cdev::Tty, 0o666),
    (b"zero", 6, Cdev::Zero, 0o666),
];

/// The first file number of the console's other names, which the system
/// policy gives (FreeBSD's `ttyu0`, `ttyv0`).
const ALIAS_BASE: u64 = 7;

/// Every device: the kernel's, then the console's other names.
fn devices() -> impl Iterator<Item = (&'static [u8], u64, Cdev, u16)> {
    let aliases = crate::policy::policy().console_aliases();
    DEVICES.iter().copied().chain(
        aliases.iter().zip(ALIAS_BASE..).map(|(&name, ino)| (name, ino, Cdev::Console, 0o600)),
    )
}

fn device(ino: u64) -> Option<(&'static [u8], Cdev, u16)> {
    devices().find(|d| d.1 == ino).map(|(n, _, c, m)| (n, c, m))
}

/// The device file system.
pub struct Devfs;

/// The instance mounted on `/dev`.
pub static DEVFS: Devfs = Devfs;

impl VfsOps for Devfs {
    fn root(&self) -> u64 {
        ROOT
    }

    fn getattr(&self, ino: u64) -> KResult<VAttr> {
        let (kind, perm, nlink) = if ino == ROOT {
            (VType::Dir, 0o555, 2)
        } else {
            let (_, _, perm) = device(ino).ok_or(Errno::ENOENT)?;
            (VType::Chr, perm, 1)
        };
        let t = VTime::default();
        Ok(VAttr {
            kind,
            perm,
            nlink,
            uid: 0,
            gid: 0,
            ino,
            size: 0,
            blocks: 0,
            blksize: 4096,
            rdev: if kind == VType::Chr { ino } else { 0 },
            atime: t,
            mtime: t,
            ctime: t,
            birthtime: t,
            flags: 0,
            generation: 0,
        })
    }

    fn lookup(&self, dir: u64, name: &[u8]) -> KResult<u64> {
        if dir != ROOT {
            return Err(Errno::ENOTDIR);
        }
        if name == b"." || name == b".." {
            return Ok(ROOT);
        }
        devices().find(|d| d.0 == name).map(|d| d.1).ok_or(Errno::ENOENT)
    }

    fn read(&self, _ino: u64, _off: u64, _buf: &mut [u8]) -> KResult<usize> {
        // Devices are read through the file `devopen` gives.
        Err(Errno::EINVAL)
    }

    fn readlink(&self, _ino: u64, _buf: &mut [u8]) -> KResult<usize> {
        Err(Errno::EINVAL)
    }

    fn readdir(&self, dir: u64, off: u64, f: &mut dyn FnMut(&DirEnt<'_>) -> bool) -> KResult<u64> {
        if dir != ROOT {
            return Err(Errno::ENOTDIR);
        }
        // Offsets are entry indexes: ".", "..", then the devices.
        let mut i = off;
        loop {
            let ent = match i {
                0 => DirEnt { ino: ROOT, kind: Some(VType::Dir), name: b".", next: 1 },
                1 => DirEnt { ino: ROOT, kind: Some(VType::Dir), name: b"..", next: 2 },
                n => {
                    let Some((name, ino, _, _)) = usize::try_from(n - 2).ok().and_then(|k| devices().nth(k)) else {
                        return Ok(i);
                    };
                    DirEnt { ino, kind: Some(VType::Chr), name, next: n + 1 }
                }
            };
            if !f(&ent) {
                return Ok(i);
            }
            i += 1;
        }
    }

    fn statfs(&self) -> StatFs {
        StatFs { bsize: 512, iosize: 4096, namemax: 255, ..StatFs::default() }
    }
}

/// Opens device `ino` of the device file system for `p` (`devfs_open`):
/// what the file is.
pub fn devopen(p: &Proc, ino: u64) -> KResult<FileKind> {
    let (_, cdev, _) = device(ino).ok_or(Errno::ENXIO)?;
    Ok(match cdev {
        Cdev::Console => FileKind::Console(ino),
        // `/dev/tty`: the controlling terminal (`cttyopen`), if any.
        Cdev::Tty if crate::tty::tty_is_ctty(p.sid()) => FileKind::Console(crate::tty::tty_ctty_dev()),
        Cdev::Tty => return Err(Errno::ENXIO),
        Cdev::Null => FileKind::Null,
        Cdev::Zero => FileKind::Zero,
    })
}

/// The name of device `rdev` (`devtoname`, for `kern.devname`).
pub fn devname(rdev: u64) -> Option<&'static [u8]> {
    device(rdev).map(|(n, _, _)| n)
}
