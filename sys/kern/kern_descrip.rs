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
//! A file is the console terminal (which `init` gets as its descriptors
//! 0, 1 and 2, as FreeBSD opens `/dev/console` for it), a vnode, an end of
//! a pipe, or `/dev/null` or `/dev/zero`.
//!
//! TODO(fd): sockets, `fcntl` locks.

use framework::mm::{KArc, KVec, M_WAITOK, MallocType};
use framework::sync::Mutex;
use sys::errno::{Errno, KResult};

use crate::kern_clock::Timespec;
use crate::sys_pipe::{Pipe, PipeEnd};
use crate::vnode::{VTime, VType, Vnode};

/// Open files and descriptor tables (`M_FILEDESC`, `M_FILE`).
static M_FILEDESC: MallocType = MallocType::new("filedesc");

/// Most descriptors a process may have open (`maxfilesperproc`).
pub const MAXFILES: usize = 1024;

/// What an open file is (FreeBSD's `DTYPE_*` and `fileops`).
#[derive(Clone, Debug)]
pub enum FileKind {
    /// The console terminal, opened through this device of the device
    /// file system (its `st_rdev`: `console`, `ttyu0`, ...).
    Console(u64),
    /// A file of a file system (`DTYPE_VNODE`).
    Vnode(Vnode),
    /// An end of a pipe (`DTYPE_PIPE`).
    Pipe(KArc<Pipe>, PipeEnd),
    /// `/dev/null`: reads nothing, writes everything.
    Null,
    /// `/dev/zero`: reads zeros, writes everything.
    Zero,
}

impl FileKind {
    /// The vnode, if it is one.
    pub fn vnode(&self) -> Option<Vnode> {
        match self {
            FileKind::Vnode(vp) => Some(*vp),
            _ => None,
        }
    }
}

/// How a file was opened (`f_flag`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FileFlags {
    /// Open for reading (`FREAD`).
    pub read: bool,
    /// Open for writing (`FWRITE`).
    pub write: bool,
    /// Writes go to the end (`O_APPEND`).
    pub append: bool,
    /// Non-blocking I/O (`O_NONBLOCK`).
    pub nonblock: bool,
}

/// An open file (`struct file`).
pub struct File {
    /// What it is.
    pub kind: FileKind,
    /// How it was opened.
    pub flags: Mutex<FileFlags>,
    /// The offset of the next read or write (`f_offset`).
    pub offset: Mutex<u64>,
}

impl core::fmt::Debug for File {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("File").field("kind", &self.kind).finish_non_exhaustive()
    }
}

impl Drop for File {
    /// The last descriptor of it is gone (`closef`).
    fn drop(&mut self) {
        if let FileKind::Pipe(pipe, end) = &self.kind {
            pipe.close(*end);
        }
    }
}

impl File {
    /// A new open file.
    pub fn new(kind: FileKind, flags: FileFlags) -> KResult<KArc<File>> {
        Ok(KArc::new(File { kind, flags: Mutex::new(flags), offset: Mutex::new(0) }, &M_FILEDESC, M_WAITOK)?)
    }
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

impl From<VType> for FileType {
    fn from(t: VType) -> Self {
        match t {
            VType::Reg => FileType::Regular,
            VType::Dir => FileType::Directory,
            VType::Lnk => FileType::Symlink,
            VType::Chr => FileType::CharDevice,
            VType::Blk => FileType::BlockDevice,
            VType::Fifo => FileType::Fifo,
            VType::Sock => FileType::Socket,
        }
    }
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
    /// File flags (`chflags`).
    pub flags: u32,
    /// Generation number.
    pub generation: u32,
}

/// A slot of the table: the file and its close-on-exec flag
/// (`struct filedescent`).
#[derive(Clone)]
struct FdEntry {
    file: KArc<File>,
    cloexec: bool,
}

/// A process's descriptor table (`struct filedesc`).
pub struct Filedesc {
    files: Mutex<KVec<Option<FdEntry>>>,
}

impl Filedesc {
    /// A table with the console as descriptors 0, 1 and 2.
    pub fn console() -> KResult<Self> {
        let cons = File::new(FileKind::Console(crate::devfs::CONSOLE_DEV), FileFlags { read: true, write: true, ..FileFlags::default() })?;
        let mut files = KVec::with_capacity(3, &M_FILEDESC, M_WAITOK)?;
        for _ in 0..3 {
            files.push(Some(FdEntry { file: cons.clone(), cloexec: false }), M_WAITOK)?;
        }
        Ok(Self { files: Mutex::new(files) })
    }

    /// A copy for a child (`fdcopy`): the same open files, shared.
    pub fn fork(&self) -> KResult<Self> {
        let files = self.files.lock();
        let mut copy = KVec::with_capacity(files.len(), &M_FILEDESC, M_WAITOK)?;
        for e in files.iter() {
            copy.push(e.clone(), M_WAITOK)?;
        }
        Ok(Self { files: Mutex::new(copy) })
    }

    fn index(fd: i32) -> KResult<usize> {
        usize::try_from(fd).map_err(|_| Errno::EBADF)
    }

    /// The file open as `fd` (`fget`). The table's lock is not held after,
    /// so the caller may sleep on the file.
    pub fn get(&self, fd: i32) -> KResult<KArc<File>> {
        let i = Self::index(fd)?;
        self.files.lock().get(i).and_then(|e| e.as_ref().map(|e| e.file.clone())).ok_or(Errno::EBADF)
    }

    /// Puts `file` in the lowest free slot at or above `min` (`fdalloc`).
    pub fn alloc(&self, file: KArc<File>, min: i32, cloexec: bool) -> KResult<i32> {
        let min = usize::try_from(min).map_err(|_| Errno::EINVAL)?;
        if min >= MAXFILES {
            return Err(Errno::EINVAL);
        }
        let mut files = self.files.lock();
        while files.len() < min {
            files.push(None, M_WAITOK)?;
        }
        let i = match files.get(min..).and_then(|f| f.iter().position(Option::is_none)) {
            Some(n) => min + n,
            None if files.len() < MAXFILES => {
                files.push(None, M_WAITOK)?;
                files.len() - 1
            }
            None => return Err(Errno::EMFILE),
        };
        let slot = files.get_mut(i).ok_or(Errno::EMFILE)?;
        *slot = Some(FdEntry { file, cloexec });
        i32::try_from(i).map_err(|_| Errno::EMFILE)
    }

    /// Makes `new` a copy of `old` (`dup2`), closing what `new` was.
    pub fn dup2(&self, old: i32, new: i32, cloexec: bool) -> KResult<i32> {
        let file = self.get(old)?;
        let i = Self::index(new)?;
        if i >= MAXFILES {
            return Err(Errno::EBADF);
        }
        let mut files = self.files.lock();
        while files.len() <= i {
            files.push(None, M_WAITOK)?;
        }
        let slot = files.get_mut(i).ok_or(Errno::EBADF)?;
        // The file it replaces is dropped after the lock (`closef`).
        let _old = slot.replace(FdEntry { file, cloexec });
        Ok(new)
    }

    /// The close-on-exec flag of `fd` (`FD_CLOEXEC`).
    pub fn cloexec(&self, fd: i32) -> KResult<bool> {
        let i = Self::index(fd)?;
        self.files.lock().get(i).and_then(Option::as_ref).map(|e| e.cloexec).ok_or(Errno::EBADF)
    }

    /// Sets the close-on-exec flag of `fd`.
    pub fn set_cloexec(&self, fd: i32, on: bool) -> KResult<()> {
        let i = Self::index(fd)?;
        let mut files = self.files.lock();
        let e = files.get_mut(i).and_then(Option::as_mut).ok_or(Errno::EBADF)?;
        e.cloexec = on;
        Ok(())
    }

    /// Closes `fd` (`kern_close`).
    pub fn close(&self, fd: i32) -> KResult<()> {
        let i = Self::index(fd)?;
        let old = {
            let mut files = self.files.lock();
            let slot = files.get_mut(i).ok_or(Errno::EBADF)?;
            slot.take()
        };
        old.map(drop).ok_or(Errno::EBADF)
    }

    /// Closes descriptors `lo..=hi`, or marks them close-on-exec
    /// (`kern_close_range`).
    pub fn close_range(&self, lo: usize, hi: usize, cloexec: bool) {
        let mut files = self.files.lock();
        for slot in files.iter_mut().skip(lo).take(hi.saturating_sub(lo).saturating_add(1)) {
            if cloexec {
                if let Some(e) = slot.as_mut() {
                    e.cloexec = true;
                }
            } else {
                *slot = None;
            }
        }
    }

    /// Closes the descriptors marked close-on-exec (`fdcloseexec`).
    pub fn close_on_exec(&self) {
        let mut files = self.files.lock();
        for slot in files.iter_mut() {
            if slot.as_ref().is_some_and(|e| e.cloexec) {
                *slot = None;
            }
        }
    }

    /// Closes every descriptor (`fdescfree`, at exit).
    pub fn close_all(&self) {
        let old = core::mem::replace(&mut *self.files.lock(), KVec::new(&M_FILEDESC));
        drop(old);
    }
}

fn timespec(t: VTime) -> Timespec {
    Timespec { sec: t.sec, nsec: t.nsec }
}

/// Status of a vnode (`vn_stat`).
pub fn vn_stat(vp: &Vnode) -> KResult<Stat> {
    let a = vp.getattr()?;
    Ok(Stat {
        dev: vp.mp.ops.statfs().fsid,
        ino: a.ino,
        nlink: a.nlink,
        kind: a.kind.into(),
        perm: a.perm,
        uid: a.uid,
        gid: a.gid,
        rdev: a.rdev,
        atime: timespec(a.atime),
        mtime: timespec(a.mtime),
        ctime: timespec(a.ctime),
        birthtime: timespec(a.birthtime),
        size: i64::try_from(a.size).unwrap_or(i64::MAX),
        blocks: i64::try_from(a.blocks).unwrap_or(i64::MAX),
        blksize: i32::try_from(a.blksize).unwrap_or(i32::MAX),
        flags: a.flags,
        generation: a.generation,
    })
}

/// Status of an open file (`kern_fstat`).
pub fn file_stat(f: &File) -> KResult<Stat> {
    let zero = Timespec { sec: 0, nsec: 0 };
    let dev = |ino, perm| Stat {
        dev: 0,
        ino,
        nlink: 1,
        kind: FileType::CharDevice,
        perm,
        uid: 0,
        gid: 0,
        rdev: ino,
        atime: zero,
        mtime: zero,
        ctime: zero,
        birthtime: zero,
        size: 0,
        blocks: 0,
        blksize: 4096,
        flags: 0,
        generation: 0,
    };
    match &f.kind {
        FileKind::Null => Ok(dev(4, 0o666)),
        FileKind::Zero => Ok(dev(6, 0o666)),
        FileKind::Pipe(pipe, _) => Ok(Stat {
            kind: FileType::Fifo,
            size: i64::try_from(pipe.len()).unwrap_or(0),
            blksize: 4096,
            ..dev(pipe.ino, 0o600)
        }),
        // FreeBSD's /dev/console: crw------- root wheel.
        FileKind::Console(rdev) => Ok(Stat {
            dev: 0,
            ino: *rdev,
            nlink: 1,
            kind: FileType::CharDevice,
            perm: 0o600,
            uid: 0,
            gid: 0,
            rdev: *rdev,
            atime: zero,
            mtime: zero,
            ctime: zero,
            birthtime: zero,
            size: 0,
            blocks: 0,
            blksize: 4096,
            flags: 0,
            generation: 0,
        }),
        FileKind::Vnode(vp) => vn_stat(vp),
    }
}
