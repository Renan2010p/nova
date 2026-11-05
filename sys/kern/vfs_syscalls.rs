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


//! File system system calls (FreeBSD's `kern/vfs_syscalls.c`, and the
//! name-to-file half of `vfs_vnops.c`), on whatever file system the root
//! is. Paths are bytes, as the ABI copied them in.
//!
//! The root is mounted read-only, so asking to write, create or remove
//! is `EROFS`.

use framework::mm::{KArc, KVec, M_WAITOK};
use sys::errno::{Errno, KResult};

use crate::kern_descrip::{File, FileFlags, FileKind, Stat, vn_stat};
use crate::kern_malloc::M_TEMP;
use crate::kern_proc::Proc;
use crate::vfs_lookup::{MAXPATHLEN, NAME_MAX, namei};
use crate::vfs_mount::rootvnode;
use crate::vnode::{DirEnt, Mount, PathConf, StatFs, VType, Vnode};

/// Where a relative path starts (`AT_FDCWD` or a directory descriptor).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AtFd {
    /// The current directory.
    Cwd,
    /// The directory open as this descriptor.
    Fd(i32),
}

/// How a file is opened (`open`'s flags, by name).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OpenFlags {
    /// For reading.
    pub read: bool,
    /// For writing.
    pub write: bool,
    /// Writes append.
    pub append: bool,
    /// Create it if absent (`O_CREAT`).
    pub create: bool,
    /// With `create`: fail if it exists (`O_EXCL`).
    pub excl: bool,
    /// Truncate it (`O_TRUNC`).
    pub trunc: bool,
    /// It must be a directory (`O_DIRECTORY`).
    pub directory: bool,
    /// Do not follow a final symbolic link (`O_NOFOLLOW`).
    pub nofollow: bool,
    /// Close it on `exec` (`O_CLOEXEC`).
    pub cloexec: bool,
    /// Non-blocking I/O (`O_NONBLOCK`).
    pub nonblock: bool,
}

/// The root directory, or `ENOENT` without a root file system.
fn root() -> KResult<Vnode> {
    rootvnode().ok_or(Errno::ENOENT)
}

/// The directory a relative path starts from.
fn at_dir(p: &Proc, at: AtFd) -> KResult<Vnode> {
    match at {
        AtFd::Cwd => p.cwd.lock().ok_or(Errno::ENOENT),
        AtFd::Fd(fd) => match p.fd.get(fd)?.kind {
            FileKind::Vnode(vp) => Ok(vp),
            FileKind::Console => Err(Errno::ENOTDIR),
        },
    }
}

/// Looks `path` up for `p` from `at` (`namei` with `NDINIT_AT`).
pub fn namei_at(p: &Proc, at: AtFd, path: &[u8], follow: bool) -> KResult<Vnode> {
    let root = root()?;
    let dir = if path.first() == Some(&b'/') { root } else { at_dir(p, at)? };
    namei(root, dir, path, follow)
}

/// Opens `path` (`kern_openat`); returns the descriptor.
pub fn kern_openat(p: &Proc, at: AtFd, path: &[u8], flags: OpenFlags) -> KResult<i32> {
    let vp = match namei_at(p, at, path, !flags.nofollow) {
        Ok(vp) => vp,
        // Creating needs a writable file system.
        Err(Errno::ENOENT) if flags.create => return Err(Errno::EROFS),
        Err(e) => return Err(e),
    };
    if flags.create && flags.excl {
        return Err(Errno::EEXIST);
    }
    let attr = vp.getattr()?;
    match attr.kind {
        VType::Lnk => return Err(Errno::ELOOP), // O_NOFOLLOW on a link
        VType::Dir if flags.write => return Err(Errno::EISDIR),
        k if flags.directory && k != VType::Dir => return Err(Errno::ENOTDIR),
        _ => {}
    }
    if (flags.write || flags.trunc) && vp.mp.rdonly {
        return Err(Errno::EROFS);
    }
    let file = File::new(
        FileKind::Vnode(vp),
        FileFlags { read: flags.read, write: flags.write, append: flags.append, nonblock: flags.nonblock },
    )?;
    p.fd.alloc(file, 0, flags.cloexec)
}

/// The status of a path (`kern_statat`); with `empty_path` and an empty
/// path, of the descriptor itself (`AT_EMPTY_PATH`).
pub fn kern_statat(p: &Proc, at: AtFd, path: &[u8], follow: bool, empty_path: bool) -> KResult<Stat> {
    if path.is_empty() && empty_path {
        return match at {
            AtFd::Fd(fd) => crate::sys_generic::kern_fstat(p, fd),
            AtFd::Cwd => vn_stat(&at_dir(p, at)?),
        };
    }
    vn_stat(&namei_at(p, at, path, follow)?)
}

/// What `access` asks (`R_OK`, `W_OK`, `X_OK`; none: existence).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AccessMode {
    /// Readable.
    pub read: bool,
    /// Writable.
    pub write: bool,
    /// Executable (searchable, for a directory).
    pub exec: bool,
}

/// Checks access to a path (`kern_accessat`), as root: anything may be
/// read; writing needs a writable file system; executing needs an execute
/// bit (or a directory).
pub fn kern_accessat(p: &Proc, at: AtFd, path: &[u8], mode: AccessMode, follow: bool) -> KResult<()> {
    let vp = namei_at(p, at, path, follow)?;
    let attr = vp.getattr()?;
    if mode.write && vp.mp.rdonly {
        return Err(Errno::EROFS);
    }
    if mode.exec && attr.kind != VType::Dir && attr.perm & 0o111 == 0 {
        return Err(Errno::EACCES);
    }
    Ok(())
}

/// Reads a symbolic link into `buf` (`kern_readlinkat`); returns its
/// length (not NUL-terminated, cut at `buf`'s size).
pub fn kern_readlinkat(p: &Proc, at: AtFd, path: &[u8], buf: &mut [u8]) -> KResult<usize> {
    let vp = namei_at(p, at, path, false)?;
    if vp.getattr()?.kind != VType::Lnk {
        return Err(Errno::EINVAL);
    }
    let mut target = [0u8; MAXPATHLEN];
    let n = vp.mp.ops.readlink(vp.ino, &mut target)?;
    let n = n.min(buf.len());
    buf.get_mut(..n).ok_or(Errno::EINVAL)?.copy_from_slice(target.get(..n).ok_or(Errno::EIO)?);
    Ok(n)
}

fn set_cwd(p: &Proc, vp: Vnode) -> KResult<()> {
    if vp.getattr()?.kind != VType::Dir {
        return Err(Errno::ENOTDIR);
    }
    *p.cwd.lock() = Some(vp);
    Ok(())
}

/// Changes the current directory (`kern_chdir`).
pub fn kern_chdir(p: &Proc, path: &[u8]) -> KResult<()> {
    set_cwd(p, namei_at(p, AtFd::Cwd, path, true)?)
}

/// Changes the current directory to an open one (`sys_fchdir`).
pub fn kern_fchdir(p: &Proc, fd: i32) -> KResult<()> {
    set_cwd(p, at_dir(p, AtFd::Fd(fd))?)
}

/// The name of `child` in directory `dir` (`vn_vptocnp`), into `name`.
fn name_in(dir: Vnode, child: Vnode, name: &mut [u8; NAME_MAX]) -> KResult<usize> {
    let mut len = None;
    dir.mp.ops.readdir(dir.ino, 0, &mut |e: &DirEnt<'_>| {
        if e.ino == child.ino && e.name != b"." && e.name != b".." {
            let n = e.name.len().min(NAME_MAX);
            if let (Some(dst), Some(src)) = (name.get_mut(..n), e.name.get(..n)) {
                dst.copy_from_slice(src);
                len = Some(n);
            }
            return false;
        }
        true
    })?;
    len.ok_or(Errno::ENOENT)
}

/// The current directory's path (`kern___getcwd`), found by walking up
/// through `..` and naming each directory in its parent.
pub fn kern_getcwd(p: &Proc) -> KResult<KVec<u8>> {
    let root = root()?;
    let mut cur = p.cwd.lock().ok_or(Errno::ENOENT)?;
    // Built from the end of `buf` towards its start.
    let mut buf = [0u8; MAXPATHLEN];
    let mut start = buf.len();
    let mut name = [0u8; NAME_MAX];
    while !cur.same(&root) {
        let parent = Vnode { mp: cur.mp, ino: cur.mp.ops.lookup(cur.ino, b"..")? };
        let n = name_in(parent, cur, &mut name)?;
        if start < n + 1 {
            return Err(Errno::ENAMETOOLONG);
        }
        start -= n;
        buf.get_mut(start..start + n).ok_or(Errno::ERANGE)?.copy_from_slice(name.get(..n).ok_or(Errno::EIO)?);
        start -= 1;
        *buf.get_mut(start).ok_or(Errno::ERANGE)? = b'/';
        cur = parent;
    }
    let mut out = KVec::with_capacity(MAXPATHLEN, &M_TEMP, M_WAITOK)?;
    out.extend_from_slice(if start == buf.len() { b"/" } else { buf.get(start..).unwrap_or(b"/") }, M_WAITOK)?;
    Ok(out)
}

/// Reads entries of the directory open as `fd` (`kern_getdirentries`):
/// `f` takes each and returns false when it has no room for it; the
/// offset moves past those taken. Returns the offset before the read
/// (`basep`).
pub fn kern_getdirentries(p: &Proc, fd: i32, f: &mut dyn FnMut(&DirEnt<'_>) -> bool) -> KResult<u64> {
    let file: KArc<File> = p.fd.get(fd)?;
    let FileKind::Vnode(vp) = file.kind else { return Err(Errno::EINVAL) };
    if vp.getattr()?.kind != VType::Dir {
        return Err(Errno::EINVAL);
    }
    let mut off = file.offset.lock();
    let base = *off;
    *off = vp.mp.ops.readdir(vp.ino, base, f)?;
    Ok(base)
}

/// The file system holding a path or an open file (`kern_statfs`,
/// `kern_fstatfs`).
pub fn kern_statfs(p: &Proc, at: Result<i32, &[u8]>) -> KResult<(StatFs, &'static Mount)> {
    let vp = match at {
        Ok(fd) => match p.fd.get(fd)?.kind {
            FileKind::Vnode(vp) => vp,
            // The console lives on devfs, which Nova does not have yet.
            FileKind::Console => return Err(Errno::EINVAL),
        },
        Err(path) => namei_at(p, AtFd::Cwd, path, true)?,
    };
    Ok((vp.mp.ops.statfs(), vp.mp))
}

/// The mounted file systems (`getfsstat`): the root, if any.
pub fn kern_getfsstat() -> Option<(StatFs, &'static Mount)> {
    rootvnode().map(|vp| (vp.mp.ops.statfs(), vp.mp))
}

/// A limit of the file system holding a path or an open file
/// (`kern_pathconf`, `kern_fpathconf`); `follow`: of a final link's
/// target (`pathconf`) or the link itself (`lpathconf`).
pub fn kern_pathconf(p: &Proc, at: Result<i32, &[u8]>, follow: bool, name: PathConf) -> KResult<i64> {
    let vp = match at {
        Ok(fd) => match p.fd.get(fd)?.kind {
            FileKind::Vnode(vp) => vp,
            FileKind::Console => return Err(Errno::EINVAL),
        },
        Err(path) => namei_at(p, AtFd::Cwd, path, follow)?,
    };
    vp.mp.ops.pathconf(name)
}

/// Copies between two open files in the kernel (`kern_copy_file_range`):
/// both must be regular files of a file system, else `EINVAL` (and the
/// program copies by itself, as `cat` does to a terminal).
pub fn kern_copy_file_range(p: &Proc, infd: i32, outfd: i32) -> KResult<u64> {
    let (inf, outf) = (p.fd.get(infd)?, p.fd.get(outfd)?);
    let (FileKind::Vnode(src), FileKind::Vnode(dst)) = (inf.kind, outf.kind) else { return Err(Errno::EINVAL) };
    if src.getattr()?.kind != VType::Reg || dst.getattr()?.kind != VType::Reg {
        return Err(Errno::EINVAL);
    }
    if !inf.flags.lock().read || !outf.flags.lock().write {
        return Err(Errno::EBADF);
    }
    // A writable file cannot be open on a read-only mount.
    Err(Errno::EROFS)
}
