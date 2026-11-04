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

//! File descriptors (FreeBSD's `kern/kern_descrip.c`): a process's table
//! of open files, and what can be asked of a file.
//!
//! Today the only file is the console terminal, which `init` gets as its
//! descriptors 0, 1 and 2 (FreeBSD opens `/dev/console` for it).
//!
//! TODO(fd): `open` (with the VFS), `dup`, `dup2`, `fcntl`, close-on-exec,
//! pipes.

use framework::mm::{KArc, KVec, M_WAITOK, MallocType};
use framework::sync::Mutex;
use sys::errno::{Errno, KResult};

use crate::kern_clock::Timespec;

/// Open files and descriptor tables (`M_FILEDESC`, `M_FILE`).
static M_FILEDESC: MallocType = MallocType::new("filedesc");

/// What an open file is (FreeBSD's `DTYPE_*` and `fileops`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    /// The console terminal.
    Console,
}

/// An open file (`struct file`).
#[derive(Debug)]
pub struct File {
    /// What it is.
    pub kind: FileKind,
}

/// The type of a file (`S_IFMT`, by name).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileType {
    /// A regular file.
    Regular,
    /// A directory.
    Directory,
    /// A character device.
    CharDevice,
    /// A block device.
    BlockDevice,
    /// A named pipe.
    Fifo,
    /// A symbolic link.
    Symlink,
    /// A socket.
    Socket,
}

/// What `stat` reports (`struct stat`, by name).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stat {
    /// Device of the file system.
    pub dev: u64,
    /// File number.
    pub ino: u64,
    /// Hard links.
    pub nlink: u64,
    /// Type.
    pub kind: FileType,
    /// Permission bits (`07777`).
    pub perm: u16,
    /// Owner.
    pub uid: u32,
    /// Group.
    pub gid: u32,
    /// Device, for a device file.
    pub rdev: u64,
    /// Last access.
    pub atime: Timespec,
    /// Last modification.
    pub mtime: Timespec,
    /// Last status change.
    pub ctime: Timespec,
    /// Creation.
    pub birthtime: Timespec,
    /// Size in bytes.
    pub size: i64,
    /// Blocks allocated, of 512 bytes.
    pub blocks: i64,
    /// Preferred I/O size.
    pub blksize: i32,
}

/// A process's descriptor table (`struct filedesc`).
pub struct Filedesc {
    files: Mutex<KVec<Option<KArc<File>>>>,
}

impl Filedesc {
    /// A table with the console as descriptors 0, 1 and 2.
    pub fn console() -> KResult<Self> {
        let cons = KArc::new(File { kind: FileKind::Console }, &M_FILEDESC, M_WAITOK)?;
        let mut files = KVec::with_capacity(3, &M_FILEDESC, M_WAITOK)?;
        for _ in 0..3 {
            files.push(Some(cons.clone()), M_WAITOK)?;
        }
        Ok(Self { files: Mutex::new(files) })
    }

    /// The file open as `fd` (`fget`). The table's lock is not held after,
    /// so the caller may sleep on the file.
    pub fn get(&self, fd: i32) -> KResult<KArc<File>> {
        let i = usize::try_from(fd).map_err(|_| Errno::EBADF)?;
        self.files.lock().get(i).and_then(Option::clone).ok_or(Errno::EBADF)
    }

    /// Closes `fd` (`kern_close`).
    pub fn close(&self, fd: i32) -> KResult<()> {
        let i = usize::try_from(fd).map_err(|_| Errno::EBADF)?;
        let mut files = self.files.lock();
        let slot = files.get_mut(i).ok_or(Errno::EBADF)?;
        slot.take().map(drop).ok_or(Errno::EBADF)
    }
}

/// Status of an open file (`kern_fstat`).
pub fn file_stat(f: &File) -> Stat {
    let zero = Timespec { sec: 0, nsec: 0 };
    match f.kind {
        // FreeBSD's /dev/console: crw------- root wheel.
        FileKind::Console => Stat {
            dev: 0,
            ino: 0,
            nlink: 1,
            kind: FileType::CharDevice,
            perm: 0o600,
            uid: 0,
            gid: 0,
            rdev: 0,
            atime: zero,
            mtime: zero,
            ctime: zero,
            birthtime: zero,
            size: 0,
            blocks: 0,
            blksize: 4096,
        },
    }
}
