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
    AccessMode, AtFd, OpenFlags, kern_accessat, kern_chdir, kern_copy_file_range, kern_fchdir, kern_getcwd,
    kern_getdirentries, kern_getfsstat, kern_openat, kern_pathconf, kern_readlinkat, kern_statat, kern_statfs,
};
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
pub(crate) fn sys_openat(p: &Proc, fd: i32, path: u64, flags: i32) -> KResult<[u64; 2]> {
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
    let flags = MNT_LOCAL | if mp.rdonly { MNT_RDONLY } else { 0 } | if mp.on == "/" { MNT_ROOTFS } else { 0 };
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
    put(296, mp.from.as_bytes());
    put(1320, mp.on.as_bytes());
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

/// `getfsstat(buf, bufsize, mode)`: the root, if mounted.
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_getfsstat(p: &Proc, buf: u64, size: u64) -> KResult<[u64; 2]> {
    let Some((s, mp)) = kern_getfsstat() else { return Ok([0, 0]) };
    if buf != 0 && size >= u64_from_usize(STATFS_SIZE) {
        p.vm().copyout(buf, &statfs_out(&s, mp)).map_err(vm_errno)?;
    }
    Ok([1, 0])
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
pub(crate) fn sys_copy_file_range(p: &Proc, infd: i32, outfd: i32) -> KResult<[u64; 2]> {
    Ok([kern_copy_file_range(p, infd, outfd)?, 0])
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
