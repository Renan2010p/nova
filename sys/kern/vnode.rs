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


//! Vnodes and file systems, as the kernel sees them (FreeBSD's
//! `sys/vnode.h` and `sys/mount.h`): through an interface only.
//!
//! The kernel knows no file system. It knows [`VfsOps`], what every file
//! system offers (look a name up, read, list a directory, describe a
//! file), and works on [`Vnode`]s: a mount and a file number in it. Each
//! file system is a crate under `sys/compat/<abi>/fs/<name>` (a file
//! system is a format a system defines, as an ABI is), which `sys/conf`
//! registers ([`crate::vfs_mount::vfs_register`]); the root's type comes
//! from `vfs.root.mountfrom`, as a string. `checklayers` rule R19 keeps
//! file system names out of every other crate.
//!
//! TODO(vfs): writing, a name cache, several mounts (`mount(2)`, mount
//! points crossed by lookup), devices other than memory disks.

use sys::errno::KResult;

/// The type of a file (`enum vtype`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VType {
    /// A regular file.
    Reg,
    /// A directory.
    Dir,
    /// A symbolic link.
    Lnk,
    /// A character device.
    Chr,
    /// A block device.
    Blk,
    /// A named pipe.
    Fifo,
    /// A socket.
    Sock,
}

/// A time stamp of a file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VTime {
    /// Seconds since the Epoch.
    pub sec: i64,
    /// Nanoseconds.
    pub nsec: i64,
}

/// What a file system says of a file (`struct vattr`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VAttr {
    /// Its type.
    pub kind: VType,
    /// Permission bits (`07777`).
    pub perm: u16,
    /// Hard links.
    pub nlink: u64,
    /// Owner.
    pub uid: u32,
    /// Group.
    pub gid: u32,
    /// File number.
    pub ino: u64,
    /// Size in bytes.
    pub size: u64,
    /// Space used, in 512-byte blocks.
    pub blocks: u64,
    /// Preferred I/O size.
    pub blksize: u32,
    /// Device, for a device file.
    pub rdev: u64,
    /// Last access.
    pub atime: VTime,
    /// Last modification.
    pub mtime: VTime,
    /// Last status change.
    pub ctime: VTime,
    /// Creation.
    pub birthtime: VTime,
    /// File flags (`chflags`).
    pub flags: u32,
    /// Generation number.
    pub generation: u32,
}

/// A directory entry, as [`VfsOps::readdir`] gives it.
#[derive(Clone, Copy, Debug)]
pub struct DirEnt<'a> {
    /// File number.
    pub ino: u64,
    /// Its type, if the directory records it.
    pub kind: Option<VType>,
    /// Its name.
    pub name: &'a [u8],
    /// The directory offset of the entry after it.
    pub next: u64,
}

/// What a file system says of itself (`struct statfs`, the part a file
/// system fills in).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StatFs {
    /// Fundamental block size.
    pub bsize: u64,
    /// Optimal transfer size.
    pub iosize: u64,
    /// Total blocks.
    pub blocks: u64,
    /// Free blocks.
    pub bfree: u64,
    /// Free blocks for unprivileged users.
    pub bavail: u64,
    /// Total files.
    pub files: u64,
    /// Free files.
    pub ffree: u64,
    /// Longest name.
    pub namemax: u32,
    /// File system id.
    pub fsid: u64,
}

/// A file system, mounted (FreeBSD's `vfsops` and `vop_vector` together:
/// the operations on the mount and on its files). Files are named by
/// their number on it. Read-only for now.
pub trait VfsOps: Sync {
    /// The root directory's number.
    fn root(&self) -> u64;
    /// What the file `ino` is (`VOP_GETATTR`).
    fn getattr(&self, ino: u64) -> KResult<VAttr>;
    /// The number of `name` in directory `dir` (`VOP_LOOKUP`); `ENOENT`
    /// if absent. `.` and `..` are looked up as any name.
    fn lookup(&self, dir: u64, name: &[u8]) -> KResult<u64>;
    /// Reads at `off` into `buf`; returns the bytes read, 0 at the end
    /// (`VOP_READ`).
    fn read(&self, ino: u64, off: u64, buf: &mut [u8]) -> KResult<usize>;
    /// The target of symbolic link `ino`, into `buf`; returns its length
    /// (`VOP_READLINK`).
    fn readlink(&self, ino: u64, buf: &mut [u8]) -> KResult<usize>;
    /// Calls `f` for each entry of directory `dir` from offset `off`,
    /// until `f` returns false or the directory ends; returns the offset
    /// to go on from (`VOP_READDIR`).
    fn readdir(&self, dir: u64, off: u64, f: &mut dyn FnMut(&DirEnt<'_>) -> bool) -> KResult<u64>;
    /// Sizes and counts (`VFS_STATFS`).
    fn statfs(&self) -> StatFs;
    /// A limit or option of the file system (`VOP_PATHCONF`); the
    /// default answers what does not depend on the format.
    fn pathconf(&self, name: PathConf) -> KResult<i64> {
        match name {
            PathConf::NameMax => Ok(i64::from(self.statfs().namemax)),
            PathConf::FileSizeBits => Ok(64),
            // No ACLs unless the file system says so.
            PathConf::AclExtended | PathConf::AclNfs4 => Ok(0),
            PathConf::LinkMax => Err(sys::errno::Errno::EINVAL),
        }
    }
}

/// What `pathconf` asks a file system (the `_PC_*` names that depend on
/// it; the others are the kernel's).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathConf {
    /// Most hard links (`_PC_LINK_MAX`).
    LinkMax,
    /// Longest name (`_PC_NAME_MAX`).
    NameMax,
    /// Bits of a file size (`_PC_FILESIZEBITS`).
    FileSizeBits,
    /// POSIX.1e ACLs (`_PC_ACL_EXTENDED`).
    AclExtended,
    /// NFSv4 ACLs (`_PC_ACL_NFS4`).
    AclNfs4,
}

/// A mounted file system (`struct mount`).
pub struct Mount {
    /// Its operations.
    pub ops: &'static dyn VfsOps,
    /// The file system's type name (`vfc_name`), as registered.
    pub fstype: &'static str,
    /// What was mounted (`f_mntfromname`), e.g. `/dev/md0`.
    pub from: &'static str,
    /// Where (`f_mntonname`).
    pub on: &'static str,
    /// Read-only.
    pub rdonly: bool,
}

impl core::fmt::Debug for Mount {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Mount").field("fstype", &self.fstype).field("from", &self.from).finish_non_exhaustive()
    }
}

/// A file: a mount and a number on it (FreeBSD's `struct vnode`, without
/// the cache: the file system is asked each time).
#[derive(Clone, Copy, Debug)]
pub struct Vnode {
    /// Its file system.
    pub mp: &'static Mount,
    /// Its number there.
    pub ino: u64,
}

impl Vnode {
    /// `VOP_GETATTR`.
    pub fn getattr(&self) -> KResult<VAttr> {
        self.mp.ops.getattr(self.ino)
    }

    /// `VOP_READ`.
    pub fn read(&self, off: u64, buf: &mut [u8]) -> KResult<usize> {
        self.mp.ops.read(self.ino, off, buf)
    }

    /// The same file.
    pub fn same(&self, other: &Vnode) -> bool {
        core::ptr::eq(self.mp, other.mp) && self.ino == other.ino
    }
}
