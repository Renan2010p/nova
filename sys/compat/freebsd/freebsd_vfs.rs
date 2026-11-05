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


//! FreeBSD's file system calls (`open`, `fstatat`, `getdirentries`,
//! `fcntl`, ...): flags and structures decoded here, the work done by the
//! kernel's VFS (`kern::vfs_syscalls`). Numbers from `tools/abidump` and
//! FreeBSD's headers (`sys/fcntl.h`, `sys/dirent.h`, `sys/mount.h`).

use framework::user::VmSpace;
use kern::kern_exec::vm_errno;
use kern::kern_proc::Proc;
use kern::sys_generic::{Whence, kern_lseek};
use kern::sys_generic::{kern_read, kern_write};
use kern::vfs_syscalls::{
    AccessMode, AtFd, OpenFlags, Target, kern_accessat, kern_chdir, kern_copy_file_range, kern_fchdir, kern_getcwd,
    kern_getdirentries, kern_getfsstat, kern_linkat, kern_mkdirat, kern_mkfifoat, kern_nmount, kern_openat,
    kern_pathconf, kern_readlinkat, kern_renameat, kern_setattr, kern_statat, kern_statfs, kern_symlinkat,
    kern_realpathat, kern_unlinkat, kern_unmount, vfs_timestamp,
};
use kern::vnode::{SetAttr, VTime};
use kern::vnode::{DirEnt, Mount, PathConf, StatFs, VType};
use libkern::conv::{u64_from_usize, usize_from_u64};
use sys::errno::{Errno, KResult};

use crate::freebsd_types::stat_out;

/// `AT_FDCWD`.
const AT_FDCWD: i32 = -100;
const AT_SYMLINK_NOFOLLOW: i32 = 0x200;
const AT_EMPTY_PATH: i32 = 0x4000;

/// Longest path (`MAXPATHLEN`).
const MAXPATHLEN: usize = 1024;

/// Copies the NUL-terminated string at `addr` into `buf` (`copyinstr`);
/// returns its length. `ENAMETOOLONG` if it does not fit.
pub(crate) fn copyinstr(vm: &VmSpace, addr: u64, buf: &mut [u8]) -> KResult<usize> {
    let mut done = 0;
    while done < buf.len() {
        // A piece up to the end of the page, which faults alone if it must.
        let at = addr + u64_from_usize(done);
        let to_page = usize_from_u64(4096 - (at % 4096));
        let n = to_page.min(buf.len() - done);
        let piece = buf.get_mut(done..done + n).ok_or(Errno::EFAULT)?;
        vm.copyin(at, piece).map_err(vm_errno)?;
        if let Some(nul) = piece.iter().position(|&c| c == 0) {
            return Ok(done + nul);
        }
        done += n;
    }
    Err(Errno::ENAMETOOLONG)
}

/// The directory a `*at` call starts from.
fn at_fd(fd: i32) -> AtFd {
    if fd == AT_FDCWD { AtFd::Cwd } else { AtFd::Fd(fd) }
}

/// `open(path, flags, mode)` and `openat(fd, path, flags, mode)`.
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_openat(p: &Proc, fd: i32, path: u64, flags: i32, mode: u64) -> KResult<[u64; 2]> {
    const O_ACCMODE: i32 = 3;
    const O_WRONLY: i32 = 1;
    const O_RDWR: i32 = 2;
    const O_NONBLOCK: i32 = 0x4;
    const O_APPEND: i32 = 0x8;
    const O_NOFOLLOW: i32 = 0x100;
    const O_CREAT: i32 = 0x200;
    const O_TRUNC: i32 = 0x400;
    const O_EXCL: i32 = 0x800;
    const O_DIRECTORY: i32 = 0x2_0000;
    const O_EXEC: i32 = 0x4_0000;
    const O_CLOEXEC: i32 = 0x10_0000;
    let mut buf = [0u8; MAXPATHLEN];
    let n = copyinstr(&p.vm(), path, &mut buf)?;
    let acc = flags & O_ACCMODE;
    if acc == O_ACCMODE {
        return Err(Errno::EINVAL);
    }
    let f = OpenFlags {
        read: acc != O_WRONLY && flags & O_EXEC == 0,
        write: acc == O_WRONLY || acc == O_RDWR,
        append: flags & O_APPEND != 0,
        create: flags & O_CREAT != 0,
        excl: flags & O_EXCL != 0,
        trunc: flags & O_TRUNC != 0,
        directory: flags & O_DIRECTORY != 0,
        nofollow: flags & O_NOFOLLOW != 0,
        cloexec: flags & O_CLOEXEC != 0,
        nonblock: flags & O_NONBLOCK != 0,
        mode: u16::try_from(mode & 0o7777).unwrap_or(0),
    };
    let fd = kern_openat(p, at_fd(fd), buf.get(..n).unwrap_or(&[]), f)?;
    Ok([u64::from(fd.unsigned_abs()), 0])
}

/// `fstatat(fd, path, sb, flag)` (and so `stat`, `lstat`).
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_fstatat(p: &Proc, fd: i32, path: u64, sb: u64, flag: i32) -> KResult<[u64; 2]> {
    let mut buf = [0u8; MAXPATHLEN];
    let vm = p.vm();
    let n = copyinstr(&vm, path, &mut buf)?;
    let follow = flag & AT_SYMLINK_NOFOLLOW == 0;
    let st = kern_statat(p, at_fd(fd), buf.get(..n).unwrap_or(&[]), follow, flag & AT_EMPTY_PATH != 0)?;
    vm.copyout(sb, &stat_out(&st)).map_err(vm_errno)?;
    Ok([0, 0])
}

/// `readlinkat(fd, path, buf, bufsize)` (and `readlink`).
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_readlinkat(p: &Proc, fd: i32, path: u64, buf: u64, size: u64) -> KResult<[u64; 2]> {
    let vm = p.vm();
    let mut name = [0u8; MAXPATHLEN];
    let n = copyinstr(&vm, path, &mut name)?;
    let mut target = [0u8; MAXPATHLEN];
    let room = usize_from_u64(size.min(u64_from_usize(target.len())));
    let len = kern_readlinkat(p, at_fd(fd), name.get(..n).unwrap_or(&[]), target.get_mut(..room).unwrap_or(&mut []))?;
    vm.copyout(buf, target.get(..len).unwrap_or(&[])).map_err(vm_errno)?;
    Ok([u64_from_usize(len), 0])
}

/// `faccessat(fd, path, mode, flag)` (and `access`, `eaccess`): as root,
/// the real and effective ids are the same.
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_faccessat(p: &Proc, fd: i32, path: u64, mode: i32, flag: i32) -> KResult<[u64; 2]> {
    const R_OK: i32 = 4;
    const W_OK: i32 = 2;
    const X_OK: i32 = 1;
    if mode & !(R_OK | W_OK | X_OK) != 0 {
        return Err(Errno::EINVAL);
    }
    let mut buf = [0u8; MAXPATHLEN];
    let n = copyinstr(&p.vm(), path, &mut buf)?;
    let m = AccessMode { read: mode & R_OK != 0, write: mode & W_OK != 0, exec: mode & X_OK != 0 };
    kern_accessat(p, at_fd(fd), buf.get(..n).unwrap_or(&[]), m, flag & AT_SYMLINK_NOFOLLOW == 0)?;
    Ok([0, 0])
}

/// `chdir(path)`.
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_chdir(p: &Proc, path: u64) -> KResult<[u64; 2]> {
    let mut buf = [0u8; MAXPATHLEN];
    let n = copyinstr(&p.vm(), path, &mut buf)?;
    kern_chdir(p, buf.get(..n).unwrap_or(&[]))?;
    Ok([0, 0])
}

/// `fchdir(fd)`.
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_fchdir(p: &Proc, fd: i32) -> KResult<[u64; 2]> {
    kern_fchdir(p, fd)?;
    Ok([0, 0])
}

/// `__getcwd(buf, size)`: the path, with its NUL.
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_getcwd(p: &Proc, buf: u64, size: u64) -> KResult<[u64; 2]> {
    let cwd = kern_getcwd(p)?;
    let need = u64_from_usize(cwd.len() + 1);
    if size < need {
        return Err(Errno::ERANGE);
    }
    let vm = p.vm();
    vm.copyout(buf, &cwd).map_err(vm_errno)?;
    vm.copyout(buf + need - 1, &[0]).map_err(vm_errno)?;
    Ok([0, 0])
}

/// `DT_*`, a directory entry's type.
fn dtype(kind: Option<VType>) -> u8 {
    match kind {
        None => 0,
        Some(VType::Fifo) => 1,
        Some(VType::Chr) => 2,
        Some(VType::Dir) => 4,
        Some(VType::Blk) => 6,
        Some(VType::Reg) => 8,
        Some(VType::Lnk) => 10,
        Some(VType::Sock) => 12,
    }
}

/// `getdirentries(fd, buf, nbytes, basep)`: `struct dirent` records, each
/// `d_fileno`, `d_off`, `d_reclen`, `d_type`, `d_namlen` and the name with
/// its NUL, padded to 8 bytes.
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_getdirentries(p: &Proc, fd: i32, buf: u64, nbytes: u64, basep: u64) -> KResult<[u64; 2]> {
    let vm = p.vm();
    let mut used = 0u64;
    let mut err = None;
    let base = kern_getdirentries(p, fd, &mut |e: &DirEnt<'_>| {
        let namlen = e.name.len();
        let reclen = (24 + namlen + 1).next_multiple_of(8);
        if used + u64_from_usize(reclen) > nbytes {
            return false;
        }
        let mut rec = [0u8; 24 + 256 + 8];
        rec[0..8].copy_from_slice(&e.ino.to_le_bytes());
        rec[8..16].copy_from_slice(&e.next.to_le_bytes());
        rec[16..18].copy_from_slice(&u16::try_from(reclen).unwrap_or(0).to_le_bytes());
        rec[18] = dtype(e.kind);
        rec[20..22].copy_from_slice(&u16::try_from(namlen).unwrap_or(0).to_le_bytes());
        if let Some(dst) = rec.get_mut(24..24 + namlen) {
            dst.copy_from_slice(e.name);
        }
        if let Err(e) = vm.copyout(buf + used, rec.get(..reclen).unwrap_or(&[])) {
            err = Some(vm_errno(e));
            return false;
        }
        used += u64_from_usize(reclen);
        true
    })?;
    if let Some(e) = err {
        return Err(e);
    }
    // Room for not even one entry.
    if used == 0 && nbytes < 32 {
        return Err(Errno::EINVAL);
    }
    if basep != 0 {
        vm.copyout(basep, &base.to_le_bytes()).map_err(vm_errno)?;
    }
    Ok([used, 0])
}

/// `lseek(fd, offset, whence)`.
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_lseek(p: &Proc, fd: i32, offset: u64, whence: i32) -> KResult<[u64; 2]> {
    let whence = match whence {
        0 => Whence::Set,
        1 => Whence::Cur,
        2 => Whence::End,
        _ => return Err(Errno::EINVAL),
    };
    let off = i64::from_le_bytes(offset.to_le_bytes());
    Ok([kern_lseek(p, fd, off, whence)?, 0])
}

/// `dup(fd)`.
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_dup(p: &Proc, fd: i32) -> KResult<[u64; 2]> {
    let new = p.fd.alloc(p.fd.get(fd)?, 0, false)?;
    Ok([u64::from(new.unsigned_abs()), 0])
}

/// `dup2(from, to)`.
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_dup2(p: &Proc, from: i32, to: i32) -> KResult<[u64; 2]> {
    if from == to {
        p.fd.get(from)?;
        return Ok([u64::from(to.unsigned_abs()), 0]);
    }
    let new = p.fd.dup2(from, to, false)?;
    Ok([u64::from(new.unsigned_abs()), 0])
}

/// `fcntl(fd, cmd, arg)`: duplicating, the close-on-exec flag, and the
/// status flags.
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_fcntl(p: &Proc, fd: i32, cmd: i32, arg: u64) -> KResult<[u64; 2]> {
    const F_DUPFD: i32 = 0;
    const F_GETFD: i32 = 1;
    const F_SETFD: i32 = 2;
    const F_GETFL: i32 = 3;
    const F_SETFL: i32 = 4;
    const F_DUP2FD: i32 = 10;
    const F_DUPFD_CLOEXEC: i32 = 17;
    const F_DUP2FD_CLOEXEC: i32 = 18;
    const FD_CLOEXEC: u64 = 1;
    const O_NONBLOCK: u64 = 0x4;
    const O_APPEND: u64 = 0x8;
    let iarg = libkern::conv::trunc_i32(arg);
    let ret = |fd: i32| Ok([u64::from(fd.unsigned_abs()), 0]);
    match cmd {
        F_DUPFD | F_DUPFD_CLOEXEC => ret(p.fd.alloc(p.fd.get(fd)?, iarg, cmd == F_DUPFD_CLOEXEC)?),
        F_DUP2FD | F_DUP2FD_CLOEXEC => {
            if iarg == fd {
                p.fd.get(fd)?;
                return ret(fd);
            }
            ret(p.fd.dup2(fd, iarg, cmd == F_DUP2FD_CLOEXEC)?)
        }
        F_GETFD => Ok([if p.fd.cloexec(fd)? { FD_CLOEXEC } else { 0 }, 0]),
        F_SETFD => {
            p.fd.set_cloexec(fd, arg & FD_CLOEXEC != 0)?;
            Ok([0, 0])
        }
        F_GETFL => {
            let f = *p.fd.get(fd)?.flags.lock();
            let acc = match (f.read, f.write) {
                (true, true) => 2,
                (false, true) => 1,
                _ => 0,
            };
            let fl = acc | if f.nonblock { O_NONBLOCK } else { 0 } | if f.append { O_APPEND } else { 0 };
            Ok([fl, 0])
        }
        F_SETFL => {
            let file = p.fd.get(fd)?;
            let mut f = file.flags.lock();
            f.nonblock = arg & O_NONBLOCK != 0;
            f.append = arg & O_APPEND != 0;
            Ok([0, 0])
        }
        _ => {
            p.fd.get(fd)?;
            Err(Errno::EINVAL)
        }
    }
}

/// `STATFS_VERSION`.
const STATFS_VERSION: u32 = 0x2014_0518;
/// `sizeof(struct statfs)`.
const STATFS_SIZE: usize = 2344;

/// `struct statfs` for a mount.
fn statfs_out(s: &StatFs, mp: &Mount) -> [u8; STATFS_SIZE] {
    const MNT_RDONLY: u64 = 0x1;
    const MNT_LOCAL: u64 = 0x1000;
    const MNT_ROOTFS: u64 = 0x4000;
    let mut b = [0u8; STATFS_SIZE];
    let mut put = |off: usize, v: &[u8]| {
        if let Some(dst) = b.get_mut(off..off + v.len()) {
            dst.copy_from_slice(v);
        }
    };
    put(0, &STATFS_VERSION.to_le_bytes());
    let flags = MNT_LOCAL | if mp.rdonly { MNT_RDONLY } else { 0 } | if mp.on.as_str() == "/" { MNT_ROOTFS } else { 0 };
    put(8, &flags.to_le_bytes());
    put(16, &s.bsize.to_le_bytes());
    put(24, &s.iosize.to_le_bytes());
    put(32, &s.blocks.to_le_bytes());
    put(40, &s.bfree.to_le_bytes());
    put(48, &s.bavail.to_le_bytes());
    put(56, &s.files.to_le_bytes());
    put(64, &s.ffree.to_le_bytes());
    put(184, &s.namemax.to_le_bytes());
    put(192, &s.fsid.to_le_bytes());
    // Names, NUL-terminated (the array is zeroed).
    put(280, mp.fstype.as_bytes().get(..15).unwrap_or(mp.fstype.as_bytes()));
    put(296, mp.from.as_str().as_bytes());
    put(1320, mp.on.as_str().as_bytes());
    b
}

/// `statfs(path, buf)` and `fstatfs(fd, buf)`.
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_statfs(p: &Proc, at: Result<i32, u64>, buf: u64) -> KResult<[u64; 2]> {
    let vm = p.vm();
    let mut name = [0u8; MAXPATHLEN];
    let target = match at {
        Ok(fd) => Ok(fd),
        Err(path) => {
            let n = copyinstr(&vm, path, &mut name)?;
            Err(name.get(..n).unwrap_or(&[]))
        }
    };
    let (s, mp) = kern_statfs(p, target)?;
    vm.copyout(buf, &statfs_out(&s, mp)).map_err(vm_errno)?;
    Ok([0, 0])
}

/// `getfsstat(buf, bufsize, mode)`: every mount, as many as fit; the
/// count of all without a buffer.
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_getfsstat(p: &Proc, buf: u64, size: u64) -> KResult<[u64; 2]> {
    let vm = p.vm();
    let room = size / u64_from_usize(STATFS_SIZE);
    let mut copied = 0u64;
    let mut err = None;
    let n = kern_getfsstat(&mut |s, mp| {
        if buf == 0 || copied >= room || err.is_some() {
            return;
        }
        if let Err(e) = vm.copyout(buf + copied * u64_from_usize(STATFS_SIZE), &statfs_out(&s, mp)) {
            err = Some(vm_errno(e));
        }
        copied += 1;
    });
    if let Some(e) = err {
        return Err(e);
    }
    Ok([if buf == 0 { u64_from_usize(n) } else { copied }, 0])
}

/// `pathconf(path, name)`, `lpathconf(path, name)` and `fpathconf(fd,
/// name)`: the kernel's limits, and the file system's.
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_pathconf(p: &Proc, at: Result<i32, u64>, follow: bool, name: i32) -> KResult<[u64; 2]> {
    let vm = p.vm();
    let mut buf = [0u8; MAXPATHLEN];
    let target = match at {
        Ok(fd) => Ok(fd),
        Err(path) => {
            let n = copyinstr(&vm, path, &mut buf)?;
            Err(buf.get(..n).unwrap_or(&[]))
        }
    };
    let fs = |name| kern_pathconf(p, target, follow, name);
    let v: i64 = match name {
        1 => fs(PathConf::LinkMax)?,
        4 => fs(PathConf::NameMax)?,
        12 => fs(PathConf::FileSizeBits)?,
        59 => fs(PathConf::AclExtended)?,
        64 => fs(PathConf::AclNfs4)?,
        // The kernel's own: `MAXPATHLEN`, `PIPE_BUF`, `_POSIX_*`.
        5 | 18 | 2 | 3 | 6 | 7 | 8 | 9 => {
            // The path must still exist.
            fs(PathConf::NameMax)?;
            match name {
                5 | 18 => 1024,
                2 | 3 => 255, // MAX_CANON, MAX_INPUT
                6 => 512,
                9 => 0xff, // _POSIX_VDISABLE
                _ => 1,    // CHOWN_RESTRICTED, NO_TRUNC
            }
        }
        _ => return Err(Errno::EINVAL),
    };
    Ok([u64::from_le_bytes(v.to_le_bytes()), 0])
}

/// `copy_file_range(infd, inoffp, outfd, outoffp, len, flags)`.
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_copy_file_range(p: &Proc, args: &[u64; 6]) -> KResult<[u64; 2]> {
    let vm = p.vm();
    let off = |ptr: u64| -> KResult<Option<u64>> {
        if ptr == 0 {
            return Ok(None);
        }
        let mut b = [0u8; 8];
        vm.copyin(ptr, &mut b).map_err(vm_errno)?;
        Ok(Some(u64::from_le_bytes(b)))
    };
    let (inoff, outoff) = (off(args[1])?, off(args[3])?);
    let infd = libkern::conv::trunc_i32(args[0]);
    let outfd = libkern::conv::trunc_i32(args[2]);
    let (n, i, o) = kern_copy_file_range(p, infd, inoff, outfd, outoff, args[4])?;
    if args[1] != 0 {
        vm.copyout(args[1], &i.to_le_bytes()).map_err(vm_errno)?;
    }
    if args[3] != 0 {
        vm.copyout(args[3], &o.to_le_bytes()).map_err(vm_errno)?;
    }
    Ok([n, 0])
}

/// Most entries of an I/O vector (`IOV_MAX`).
const IOV_MAX: u64 = 1024;

/// `readv(fd, iov, iovcnt)` and `writev(fd, iov, iovcnt)`: each piece in
/// turn, stopping at a short transfer.
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_rwv(p: &Proc, fd: i32, iov: u64, cnt: u64, write: bool) -> KResult<[u64; 2]> {
    if cnt > IOV_MAX {
        return Err(Errno::EINVAL);
    }
    let vm = p.vm();
    let mut total = 0u64;
    for i in 0..cnt {
        let mut v = [0u8; 16];
        vm.copyin(iov + 16 * i, &mut v).map_err(vm_errno)?;
        let base = u64::from_le_bytes(v[..8].try_into().map_err(|_| Errno::EFAULT)?);
        let len = u64::from_le_bytes(v[8..].try_into().map_err(|_| Errno::EFAULT)?);
        if len == 0 {
            continue;
        }
        let r = if write { kern_write(p, fd, base, len) } else { kern_read(p, fd, base, len) };
        match r {
            Ok(n) => {
                total += n;
                if n < len {
                    break;
                }
            }
            // As FreeBSD: a partial transfer is a success.
            Err(_) if total > 0 => break,
            Err(e) => return Err(e),
        }
    }
    Ok([total, 0])
}

/// A path argument, copied in.
struct PathBuf {
    buf: [u8; MAXPATHLEN],
    len: usize,
}

impl PathBuf {
    fn get(p: &Proc, addr: u64) -> KResult<Self> {
        let mut buf = [0u8; MAXPATHLEN];
        let len = copyinstr(&p.vm(), addr, &mut buf)?;
        Ok(Self { buf, len })
    }

    fn as_bytes(&self) -> &[u8] {
        self.buf.get(..self.len).unwrap_or(&[])
    }
}

/// `mkdirat(fd, path, mode)` (and `mkdir`).
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_mkdirat(p: &Proc, fd: i32, path: u64, mode: u64) -> KResult<[u64; 2]> {
    let path = PathBuf::get(p, path)?;
    kern_mkdirat(p, at_fd(fd), path.as_bytes(), u16::try_from(mode & 0o7777).unwrap_or(0))?;
    Ok([0, 0])
}

/// `mkfifoat(fd, path, mode)` (and `mkfifo`).
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_mkfifoat(p: &Proc, fd: i32, path: u64, mode: u64) -> KResult<[u64; 2]> {
    let path = PathBuf::get(p, path)?;
    kern_mkfifoat(p, at_fd(fd), path.as_bytes(), u16::try_from(mode & 0o7777).unwrap_or(0))?;
    Ok([0, 0])
}

/// `unlinkat(fd, path, flag)` (and `unlink`, `rmdir`).
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_unlinkat(p: &Proc, fd: i32, path: u64, flag: i32) -> KResult<[u64; 2]> {
    const AT_REMOVEDIR: i32 = 0x800;
    let path = PathBuf::get(p, path)?;
    kern_unlinkat(p, at_fd(fd), path.as_bytes(), flag & AT_REMOVEDIR != 0)?;
    Ok([0, 0])
}

/// `renameat(fromfd, from, tofd, to)` (and `rename`).
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_renameat(p: &Proc, ffd: i32, from: u64, tfd: i32, to: u64) -> KResult<[u64; 2]> {
    let (from, to) = (PathBuf::get(p, from)?, PathBuf::get(p, to)?);
    kern_renameat(p, at_fd(ffd), from.as_bytes(), at_fd(tfd), to.as_bytes())?;
    Ok([0, 0])
}

/// `symlinkat(target, fd, path)` (and `symlink`).
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_symlinkat(p: &Proc, target: u64, fd: i32, path: u64) -> KResult<[u64; 2]> {
    let (target, path) = (PathBuf::get(p, target)?, PathBuf::get(p, path)?);
    kern_symlinkat(p, target.as_bytes(), at_fd(fd), path.as_bytes())?;
    Ok([0, 0])
}

/// `linkat(fd1, path1, fd2, path2, flag)` (and `link`).
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_linkat(p: &Proc, fd1: i32, path1: u64, fd2: i32, path2: u64, flag: i32) -> KResult<[u64; 2]> {
    const AT_SYMLINK_FOLLOW: i32 = 0x400;
    let (a, b) = (PathBuf::get(p, path1)?, PathBuf::get(p, path2)?);
    kern_linkat(p, at_fd(fd1), a.as_bytes(), at_fd(fd2), b.as_bytes(), flag & AT_SYMLINK_FOLLOW != 0)?;
    Ok([0, 0])
}

/// The file a path or descriptor call changes: `fd` alone if `path` is
/// 0, else the path from `fd` (`AT_FDCWD` for the plain calls).
fn with_target<R>(p: &Proc, fd: i32, path: u64, follow: bool, f: impl FnOnce(Target<'_>) -> KResult<R>) -> KResult<R> {
    if path == 0 {
        return f(Target::Fd(fd));
    }
    let path = PathBuf::get(p, path)?;
    f(Target::Path(at_fd(fd), path.as_bytes(), follow))
}

/// `fchmodat(fd, path, mode, flag)`, `chmod`, `lchmod`, `fchmod` (`path`
/// 0).
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_chmod(p: &Proc, fd: i32, path: u64, mode: u64, flag: i32) -> KResult<[u64; 2]> {
    let a = SetAttr { perm: Some(u16::try_from(mode & 0o7777).unwrap_or(0)), ..SetAttr::default() };
    with_target(p, fd, path, flag & AT_SYMLINK_NOFOLLOW == 0, |t| kern_setattr(p, t, &a))?;
    Ok([0, 0])
}

/// `fchownat(fd, path, uid, gid, flag)`, `chown`, `lchown`, `fchown`:
/// `-1` leaves an id as it is.
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_chown(p: &Proc, fd: i32, path: u64, uid: u64, gid: u64, flag: i32) -> KResult<[u64; 2]> {
    let id = |v: u64| u32::try_from(v & 0xffff_ffff).ok().filter(|&x| x != u32::MAX);
    let a = SetAttr { uid: id(uid), gid: id(gid), ..SetAttr::default() };
    with_target(p, fd, path, flag & AT_SYMLINK_NOFOLLOW == 0, |t| kern_setattr(p, t, &a))?;
    Ok([0, 0])
}

/// `utimensat(fd, path, times, flag)` and `futimens(fd, times)`: two
/// `timespec`s, `UTIME_NOW` and `UTIME_OMIT` allowed; none is "now".
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_utimensat(p: &Proc, fd: i32, path: u64, times: u64, flag: i32) -> KResult<[u64; 2]> {
    const UTIME_NOW: i64 = -1;
    const UTIME_OMIT: i64 = -2;
    let now = vfs_timestamp();
    let (atime, mtime) = if times == 0 {
        (Some(now), Some(now))
    } else {
        let mut b = [0u8; 32];
        p.vm().copyin(times, &mut b).map_err(vm_errno)?;
        let ts = |o: usize| -> KResult<Option<VTime>> {
            let sec = i64::from_le_bytes(b[o..o + 8].try_into().map_err(|_| Errno::EFAULT)?);
            let nsec = i64::from_le_bytes(b[o + 8..o + 16].try_into().map_err(|_| Errno::EFAULT)?);
            match nsec {
                UTIME_NOW => Ok(Some(now)),
                UTIME_OMIT => Ok(None),
                n if (0..1_000_000_000).contains(&n) => Ok(Some(VTime { sec, nsec: n })),
                _ => Err(Errno::EINVAL),
            }
        };
        (ts(0)?, ts(16)?)
    };
    let a = SetAttr { atime, mtime, ..SetAttr::default() };
    with_target(p, fd, path, flag & AT_SYMLINK_NOFOLLOW == 0, |t| kern_setattr(p, t, &a))?;
    Ok([0, 0])
}

/// `utimes(path, times)`, `lutimes`, `futimes` (`path` 0): two
/// `timeval`s (microseconds); none is "now".
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_utimes(p: &Proc, fd: i32, path: u64, times: u64, follow: bool) -> KResult<[u64; 2]> {
    let (atime, mtime) = if times == 0 {
        let now = vfs_timestamp();
        (now, now)
    } else {
        let mut b = [0u8; 32];
        p.vm().copyin(times, &mut b).map_err(vm_errno)?;
        let tv = |o: usize| -> KResult<VTime> {
            let sec = i64::from_le_bytes(b[o..o + 8].try_into().map_err(|_| Errno::EFAULT)?);
            let usec = i64::from_le_bytes(b[o + 8..o + 16].try_into().map_err(|_| Errno::EFAULT)?);
            if !(0..1_000_000).contains(&usec) {
                return Err(Errno::EINVAL);
            }
            Ok(VTime { sec, nsec: usec * 1000 })
        };
        (tv(0)?, tv(16)?)
    };
    let a = SetAttr { atime: Some(atime), mtime: Some(mtime), ..SetAttr::default() };
    with_target(p, fd, path, follow, |t| kern_setattr(p, t, &a))?;
    Ok([0, 0])
}

/// `truncate(path, length)` and `ftruncate(fd, length)` (`path` 0).
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_truncate(p: &Proc, fd: i32, path: u64, len: u64) -> KResult<[u64; 2]> {
    if i64::try_from(len).is_err() {
        return Err(Errno::EINVAL);
    }
    if path == 0 && !p.fd.get(fd)?.flags.lock().write {
        return Err(Errno::EINVAL);
    }
    let a = SetAttr { size: Some(len), ..SetAttr::default() };
    with_target(p, fd, path, true, |t| kern_setattr(p, t, &a))?;
    Ok([0, 0])
}

/// `chflagsat(fd, path, flags, atflag)`, `chflags`, `lchflags`,
/// `fchflags` (`path` 0).
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_chflags(p: &Proc, fd: i32, path: u64, flags: u64, atflag: i32) -> KResult<[u64; 2]> {
    let a = SetAttr { flags: Some(u32::try_from(flags & 0xffff_ffff).unwrap_or(0)), ..SetAttr::default() };
    with_target(p, fd, path, atflag & AT_SYMLINK_NOFOLLOW == 0, |t| kern_setattr(p, t, &a))?;
    Ok([0, 0])
}

/// `fsync(fd)`, `fdatasync(fd)`: nothing is cached, so nothing to write
/// back; the descriptor must be valid.
pub(crate) fn sys_fsync(p: &Proc, fd: i32) -> KResult<[u64; 2]> {
    p.fd.get(fd)?;
    Ok([0, 0])
}

/// `flock(fd, operation)`: advisory locks always granted, as one process
/// at a time uses a file for now. TODO(vfs): `vop_advlock`.
pub(crate) fn sys_flock(p: &Proc, fd: i32) -> KResult<[u64; 2]> {
    p.fd.get(fd)?;
    Ok([0, 0])
}

/// `nmount(iov, iovcnt, flags)`: the `fstype` and `fspath` options of
/// the `name`/`value` pairs (`from` and the rest are ignored); memory
/// file systems only for now.
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_nmount(p: &Proc, iov: u64, cnt: u64, flags: u64) -> KResult<[u64; 2]> {
    const MNT_RDONLY: u64 = 0x1;
    if !cnt.is_multiple_of(2) || cnt > 64 {
        return Err(Errno::EINVAL);
    }
    let vm = p.vm();
    let mut fstype = [0u8; 16];
    let mut fstype_len = None;
    let mut fspath = PathBuf { buf: [0; MAXPATHLEN], len: 0 };
    let mut have_path = false;
    let mut name = [0u8; 32];
    for i in (0..cnt).step_by(2) {
        let mut v = [0u8; 32];
        vm.copyin(iov + 16 * i, &mut v).map_err(vm_errno)?;
        let nbase = u64::from_le_bytes(v[..8].try_into().map_err(|_| Errno::EFAULT)?);
        let vbase = u64::from_le_bytes(v[16..24].try_into().map_err(|_| Errno::EFAULT)?);
        let n = copyinstr(&vm, nbase, &mut name).map_err(|_| Errno::EINVAL)?;
        match &name[..n] {
            b"fstype" => fstype_len = Some(copyinstr(&vm, vbase, &mut fstype).map_err(|_| Errno::EINVAL)?),
            b"fspath" => {
                fspath.len = copyinstr(&vm, vbase, &mut fspath.buf)?;
                have_path = true;
            }
            _ => {}
        }
    }
    let (Some(tlen), true) = (fstype_len, have_path) else { return Err(Errno::EINVAL) };
    let fstype = core::str::from_utf8(&fstype[..tlen]).map_err(|_| Errno::EINVAL)?;
    kern_nmount(p, fstype, fspath.as_bytes(), flags & MNT_RDONLY != 0)?;
    Ok([0, 0])
}

/// `__realpathat(fd, path, buf, size, flags)`: the path without links,
/// with its NUL.
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_realpathat(p: &Proc, fd: i32, path: u64, buf: u64, size: u64) -> KResult<[u64; 2]> {
    let path = PathBuf::get(p, path)?;
    let out = kern_realpathat(p, at_fd(fd), path.as_bytes())?;
    let need = u64_from_usize(out.len() + 1);
    if size < need {
        return Err(Errno::ENAMETOOLONG);
    }
    let vm = p.vm();
    vm.copyout(buf, &out).map_err(vm_errno)?;
    vm.copyout(buf + need - 1, &[0]).map_err(vm_errno)?;
    Ok([0, 0])
}

/// `unmount(path, flags)`.
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_unmount(p: &Proc, path: u64) -> KResult<[u64; 2]> {
    let path = PathBuf::get(p, path)?;
    kern_unmount(p, path.as_bytes())?;
    Ok([0, 0])
}

/// `close_range(lowfd, highfd, flags)`: `CLOSE_RANGE_CLOEXEC` marks
/// instead of closing.
pub(crate) fn sys_close_range(p: &Proc, lo: u64, hi: u64, flags: i32) -> KResult<[u64; 2]> {
    const CLOSE_RANGE_CLOEXEC: i32 = 1 << 2;
    if flags & !CLOSE_RANGE_CLOEXEC != 0 || lo > hi {
        return Err(Errno::EINVAL);
    }
    let lo = usize::try_from(lo.min(u64::from(u32::MAX))).unwrap_or(usize::MAX);
    let hi = usize::try_from(hi.min(u64::from(u32::MAX))).unwrap_or(usize::MAX);
    p.fd.close_range(lo, hi, flags & CLOSE_RANGE_CLOEXEC != 0);
    Ok([0, 0])
}
