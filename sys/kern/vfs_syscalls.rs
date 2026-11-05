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
use crate::vfs_lookup::{MAXPATHLEN, NAME_MAX, namei, namei_parent};
use crate::vfs_mount::rootvnode;
use crate::vnode::{DirEnt, Mount, NewNode, Owner, PathConf, SetAttr, StatFs, VTime, VType, Vnode};


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
    /// Permissions of a file created (before the creation mask).
    pub mode: u16,
}

/// The root directory, or `ENOENT` without a root file system.
fn root() -> KResult<Vnode> {
    rootvnode().ok_or(Errno::ENOENT)
}

/// The directory a relative path starts from.
fn at_dir(p: &Proc, at: AtFd) -> KResult<Vnode> {
    match at {
        AtFd::Cwd => p.cwd.lock().ok_or(Errno::ENOENT),
        AtFd::Fd(fd) => p.fd.get(fd)?.kind.vnode().ok_or(Errno::ENOTDIR),
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
    let (vp, created) = match namei_at(p, at, path, !flags.nofollow) {
        Ok(vp) => {
            if flags.create && flags.excl {
                return Err(Errno::EEXIST);
            }
            (vp, false)
        }
        // `O_CREAT`: a new regular file, with the creation mask applied.
        Err(Errno::ENOENT) if flags.create => (create_at(p, at, path, NewNode::File, flags.mode)?, true),
        Err(e) => return Err(e),
    };
    let attr = vp.getattr()?;
    match attr.kind {
        VType::Lnk => return Err(Errno::ELOOP), // O_NOFOLLOW on a link
        VType::Dir if flags.write => return Err(Errno::EISDIR),
        k if flags.directory && k != VType::Dir => return Err(Errno::ENOTDIR),
        _ => {}
    }
    // A device of the device file system opens as the device; device
    // files elsewhere name nothing (`ENXIO`).
    let kind = match attr.kind {
        VType::Chr | VType::Blk if vp.mp.devices => crate::devfs::devopen(p, vp.ino)?,
        VType::Chr | VType::Blk | VType::Fifo | VType::Sock => return Err(Errno::ENXIO),
        _ => FileKind::Vnode(vp),
    };
    if (flags.write || flags.trunc) && vp.mp.rdonly && matches!(kind, FileKind::Vnode(_)) {
        return Err(Errno::EROFS);
    }
    if flags.trunc && flags.write && !created && attr.kind == VType::Reg && attr.size > 0 {
        vp.mp.ops.setattr(vp.ino, &SetAttr { size: Some(0), ..SetAttr::default() })?;
    }
    let file = File::new(
        kind,
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

/// The current directory's path (`kern___getcwd`).
pub fn kern_getcwd(p: &Proc) -> KResult<KVec<u8>> {
    let cwd = p.cwd.lock().ok_or(Errno::ENOENT)?;
    vn_fullpath_dir(cwd)
}

/// The absolute path of a file without symbolic links (`kern___realpathat`):
/// the directory holding it, named by walking up, then its name.
pub fn kern_realpathat(p: &Proc, at: AtFd, path: &[u8]) -> KResult<KVec<u8>> {
    let vp = namei_at(p, at, path, true)?;
    if vp.getattr()?.kind == VType::Dir {
        return vn_fullpath_dir(vp);
    }
    // A file: follow links in the last component by hand, so its
    // directory is known.
    let mut target = [0u8; MAXPATHLEN];
    let mut cur_at = at;
    let mut buf = [0u8; MAXPATHLEN];
    let mut len = path.len().min(MAXPATHLEN);
    buf.get_mut(..len).ok_or(Errno::ENAMETOOLONG)?.copy_from_slice(path.get(..len).ok_or(Errno::EINVAL)?);
    for _ in 0..crate::vfs_lookup::MAXSYMLINKS {
        let (dir, name) = parent_at_any(p, cur_at, buf.get(..len).unwrap_or(&[]))?;
        let ino = dir.mp.ops.lookup(dir.ino, name)?;
        let f = Vnode { mp: dir.mp, ino };
        if f.getattr()?.kind != VType::Lnk {
            let mut out = vn_fullpath_dir(dir)?;
            if &*out != b"/" {
                out.push(b'/', M_WAITOK)?;
            }
            out.extend_from_slice(name, M_WAITOK)?;
            return Ok(out);
        }
        let n = f.mp.ops.readlink(f.ino, &mut target)?;
        let t = target.get(..n).ok_or(Errno::EIO)?;
        if t.first() == Some(&b'/') {
            len = n;
            buf.get_mut(..n).ok_or(Errno::ENAMETOOLONG)?.copy_from_slice(t);
        } else {
            // Relative to the link's directory: its path, then the target.
            let d = vn_fullpath_dir(dir)?;
            len = d.len() + 1 + n;
            if len > MAXPATHLEN {
                return Err(Errno::ENAMETOOLONG);
            }
            buf.get_mut(..d.len()).ok_or(Errno::ENAMETOOLONG)?.copy_from_slice(&d);
            *buf.get_mut(d.len()).ok_or(Errno::ENAMETOOLONG)? = b'/';
            buf.get_mut(d.len() + 1..len).ok_or(Errno::ENAMETOOLONG)?.copy_from_slice(t);
        }
        cur_at = AtFd::Cwd;
    }
    Err(Errno::ELOOP)
}

/// [`parent_at`] for reading: a read-only file system is fine.
fn parent_at_any<'p>(p: &Proc, at: AtFd, path: &'p [u8]) -> KResult<(Vnode, &'p [u8])> {
    let root = root()?;
    let dir = if path.first() == Some(&b'/') { root } else { at_dir(p, at)? };
    namei_parent(root, dir, path)
}

/// Unmounts what is mounted on `path` (`kern_unmount`): found by its
/// name in the mount list, as FreeBSD does (`EINVAL` if nothing is);
/// anything mounted is in use for now (`EBUSY`).
pub fn kern_unmount(_p: &Proc, path: &[u8]) -> KResult<()> {
    let mut found = false;
    crate::vfs_mount::foreach_mount(|mp| found |= mp.on.as_str().as_bytes() == path);
    if found { Err(Errno::EBUSY) } else { Err(Errno::EINVAL) }
}

/// The path of a directory, found by walking up through `..` and naming
/// each directory in its parent (`vn_fullpath`).
fn vn_fullpath_dir(dir: Vnode) -> KResult<KVec<u8>> {
    let root = root()?;
    let mut cur = dir;
    // Built from the end of `buf` towards its start.
    let mut buf = [0u8; MAXPATHLEN];
    let mut start = buf.len();
    let mut name = [0u8; NAME_MAX];
    while !cur.same(&root) {
        // A mount's root stands for the directory it covers.
        if let Some(covered) = crate::vfs_mount::covered(&cur) {
            cur = covered;
            continue;
        }
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
    let vp = file.kind.vnode().ok_or(Errno::EINVAL)?;
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
        Ok(fd) => p.fd.get(fd)?.kind.vnode().ok_or(Errno::EINVAL)?,
        Err(path) => namei_at(p, AtFd::Cwd, path, true)?,
    };
    Ok((vp.mp.ops.statfs(), vp.mp))
}

/// The mounted file systems (`getfsstat`): `f` is called for each.
pub fn kern_getfsstat(f: &mut dyn FnMut(StatFs, &'static Mount)) -> usize {
    let mut n = 0;
    crate::vfs_mount::foreach_mount(|mp| {
        f(mp.ops.statfs(), mp);
        n += 1;
    });
    n
}

/// A limit of the file system holding a path or an open file
/// (`kern_pathconf`, `kern_fpathconf`); `follow`: of a final link's
/// target (`pathconf`) or the link itself (`lpathconf`).
pub fn kern_pathconf(p: &Proc, at: Result<i32, &[u8]>, follow: bool, name: PathConf) -> KResult<i64> {
    let vp = match at {
        Ok(fd) => p.fd.get(fd)?.kind.vnode().ok_or(Errno::EINVAL)?,
        Err(path) => namei_at(p, AtFd::Cwd, path, follow)?,
    };
    vp.mp.ops.pathconf(name)
}

/// Copies up to `len` bytes between two open files in the kernel
/// (`kern_copy_file_range`): from `inoff` and to `outoff` if given (the
/// files' offsets stay), else from and to the files' offsets (which
/// move). Both must be regular files of a file system, else `EINVAL`
/// (and the program copies by itself, as `cat` does to a terminal).
/// Returns the bytes copied, and the new explicit offsets.
pub fn kern_copy_file_range(
    p: &Proc,
    infd: i32,
    inoff: Option<u64>,
    outfd: i32,
    outoff: Option<u64>,
    len: u64,
) -> KResult<(u64, u64, u64)> {
    let (inf, outf) = (p.fd.get(infd)?, p.fd.get(outfd)?);
    let (Some(src), Some(dst)) = (inf.kind.vnode(), outf.kind.vnode()) else { return Err(Errno::EINVAL) };
    if src.getattr()?.kind != VType::Reg || dst.getattr()?.kind != VType::Reg {
        return Err(Errno::EINVAL);
    }
    let (rf, wf) = (*inf.flags.lock(), *outf.flags.lock());
    if !rf.read || !wf.write || wf.append {
        return Err(Errno::EBADF);
    }
    if dst.mp.rdonly {
        return Err(Errno::EROFS);
    }
    let mut ioff = match inoff {
        Some(o) => o,
        None => *inf.offset.lock(),
    };
    let mut ooff = match outoff {
        Some(o) => o,
        None => *outf.offset.lock(),
    };
    let mut chunk: KVec<u8> = KVec::with_capacity(4096, &M_TEMP, M_WAITOK)?;
    for _ in 0..4096 {
        chunk.push(0, M_WAITOK)?;
    }
    let mut done = 0u64;
    while done < len {
        let want = usize::try_from((len - done).min(4096)).map_err(|_| Errno::EINVAL)?;
        let buf = chunk.get_mut(..want).ok_or(Errno::EIO)?;
        let got = src.read(ioff, buf)?;
        if got == 0 {
            break;
        }
        let wrote = dst.mp.ops.write(dst.ino, ooff, buf.get(..got).ok_or(Errno::EIO)?)?;
        let n = u64::try_from(wrote).map_err(|_| Errno::EIO)?;
        ioff += n;
        ooff += n;
        done += n;
        if wrote < got {
            break;
        }
    }
    if inoff.is_none() {
        *inf.offset.lock() = ioff;
    }
    if outoff.is_none() {
        *outf.offset.lock() = ooff;
    }
    Ok((done, ioff, ooff))
}

/// The directory that would hold `path`, and the name in it.
fn parent_at<'p>(p: &Proc, at: AtFd, path: &'p [u8]) -> KResult<(Vnode, &'p [u8])> {
    let root = root()?;
    let dir = if path.first() == Some(&b'/') { root } else { at_dir(p, at)? };
    let (dir, name) = namei_parent(root, dir, path)?;
    if dir.mp.rdonly {
        return Err(Errno::EROFS);
    }
    Ok((dir, name))
}

/// Creates `node` at `path` with permissions `mode` less the creation
/// mask, owned by the caller (`vn_create`).
fn create_at(p: &Proc, at: AtFd, path: &[u8], node: NewNode<'_>, mode: u16) -> KResult<Vnode> {
    let (dir, name) = parent_at(p, at, path)?;
    let umask = u16::try_from(p.umask.load(core::sync::atomic::Ordering::Relaxed) & 0o777).unwrap_or(0o022);
    let cred = p.cred();
    let owner = Owner { uid: cred.euid, gid: cred.egid, perm: mode & 0o7777 & !umask };
    let ino = dir.mp.ops.create(dir.ino, name, node, owner)?;
    Ok(Vnode { mp: dir.mp, ino })
}

/// Makes a directory (`kern_mkdirat`).
pub fn kern_mkdirat(p: &Proc, at: AtFd, path: &[u8], mode: u16) -> KResult<()> {
    if namei_at(p, at, path, false).is_ok() {
        return Err(Errno::EEXIST);
    }
    create_at(p, at, path, NewNode::Dir, mode).map(drop)
}

/// Makes a symbolic link at `path` to `target` (`kern_symlinkat`).
pub fn kern_symlinkat(p: &Proc, target: &[u8], at: AtFd, path: &[u8]) -> KResult<()> {
    if namei_at(p, at, path, false).is_ok() {
        return Err(Errno::EEXIST);
    }
    create_at(p, at, path, NewNode::Symlink(target), 0o777).map(drop)
}

/// Makes a named pipe (`kern_mkfifoat`).
pub fn kern_mkfifoat(p: &Proc, at: AtFd, path: &[u8], mode: u16) -> KResult<()> {
    if namei_at(p, at, path, false).is_ok() {
        return Err(Errno::EEXIST);
    }
    create_at(p, at, path, NewNode::Fifo, mode).map(drop)
}

/// Removes a name (`kern_unlinkat`); with `dir`, an empty directory
/// (`rmdir`, `AT_REMOVEDIR`).
pub fn kern_unlinkat(p: &Proc, at: AtFd, path: &[u8], dir: bool) -> KResult<()> {
    let (parent, name) = parent_at(p, at, path)?;
    let vp = Vnode { mp: parent.mp, ino: parent.mp.ops.lookup(parent.ino, name)? };
    let kind = vp.getattr()?.kind;
    if dir && kind != VType::Dir {
        return Err(Errno::ENOTDIR);
    }
    if !dir && kind == VType::Dir {
        return Err(Errno::EPERM);
    }
    // A mount point, or a mounted file system's root, is busy.
    if crate::vfs_mount::mounted_here(&vp).is_some() || vp.same(&root()?) {
        return Err(Errno::EBUSY);
    }
    parent.mp.ops.remove(parent.ino, name, dir)
}

/// Renames `from` to `to` (`kern_renameat`), on one file system
/// (`EXDEV` otherwise).
pub fn kern_renameat(p: &Proc, from_at: AtFd, from: &[u8], to_at: AtFd, to: &[u8]) -> KResult<()> {
    let (fdir, fname) = parent_at(p, from_at, from)?;
    let (tdir, tname) = parent_at(p, to_at, to)?;
    if !core::ptr::eq(fdir.mp, tdir.mp) {
        return Err(Errno::EXDEV);
    }
    fdir.mp.ops.lookup(fdir.ino, fname)?;
    fdir.mp.ops.rename(fdir.ino, fname, tdir.ino, tname)
}

/// Makes `to` another name for `from` (`kern_linkat`), on one file
/// system; not for directories.
pub fn kern_linkat(p: &Proc, from_at: AtFd, from: &[u8], to_at: AtFd, to: &[u8], follow: bool) -> KResult<()> {
    let vp = namei_at(p, from_at, from, follow)?;
    if vp.getattr()?.kind == VType::Dir {
        return Err(Errno::EPERM);
    }
    if namei_at(p, to_at, to, false).is_ok() {
        return Err(Errno::EEXIST);
    }
    let (dir, name) = parent_at(p, to_at, to)?;
    if !core::ptr::eq(dir.mp, vp.mp) {
        return Err(Errno::EXDEV);
    }
    dir.mp.ops.link(dir.ino, name, vp.ino)
}

/// A file named by a path or by a descriptor, for the calls that change
/// attributes.
#[derive(Clone, Copy, Debug)]
pub enum Target<'a> {
    /// The file open as this descriptor.
    Fd(i32),
    /// The file at this path; `follow` a final symbolic link.
    Path(AtFd, &'a [u8], bool),
}

fn target_vnode(p: &Proc, t: Target<'_>) -> KResult<Vnode> {
    let vp = match t {
        Target::Fd(fd) => p.fd.get(fd)?.kind.vnode().ok_or(Errno::EINVAL)?,
        Target::Path(at, path, follow) => namei_at(p, at, path, follow)?,
    };
    if vp.mp.rdonly {
        return Err(Errno::EROFS);
    }
    Ok(vp)
}

/// Changes attributes of a file (`setfmode`, `setfown`, `setutimes`,
/// `kern_truncate`): as root, any.
pub fn kern_setattr(p: &Proc, t: Target<'_>, attr: &SetAttr) -> KResult<()> {
    let vp = target_vnode(p, t)?;
    if attr.size.is_some() {
        match vp.getattr()?.kind {
            VType::Dir => return Err(Errno::EISDIR),
            VType::Reg => {}
            _ => return Err(Errno::EINVAL),
        }
    }
    vp.mp.ops.setattr(vp.ino, attr)
}

/// The time now, as a file time stamp (`vfs_timestamp`).
pub fn vfs_timestamp() -> VTime {
    let t = crate::kern_clock::kern_clock_gettime(crate::kern_clock::ClockId::Realtime);
    VTime { sec: t.sec, nsec: t.nsec }
}

/// Mounts a file system of type `fstype` on directory `path`
/// (`kern_nmount`).
pub fn kern_nmount(p: &Proc, fstype: &str, path: &[u8], rdonly: bool) -> KResult<()> {
    let vp = namei_at(p, AtFd::Cwd, path, true)?;
    let path = core::str::from_utf8(path).map_err(|_| Errno::EINVAL)?;
    crate::vfs_mount::kern_mount(fstype, vp, path, rdonly)
}
