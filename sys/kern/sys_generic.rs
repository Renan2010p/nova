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

//! Generic I/O system calls (FreeBSD's `kern/sys_generic.c`), as the
//! kernel implements them; ABIs decode their arguments and call these.

use libkern::conv::{u64_from_usize, usize_from_u64};
use sys::errno::{Errno, KResult};
use sys::termios::{Termios, Winsize};

use crate::kern_descrip::{File, FileKind, Stat, file_stat};
use crate::kern_exec::vm_errno;
use crate::kern_proc::Proc;
use crate::tty;
use crate::vnode::VType;

/// Largest single transfer (`IOSIZE_MAX`).
const IOSIZE_MAX: u64 = i64::MAX.unsigned_abs();

/// Writes `nbyte` bytes from user address `buf` to descriptor `fd`
/// (`kern_writev`). Returns the bytes written.
pub fn kern_write(p: &Proc, fd: i32, buf: u64, nbyte: u64) -> KResult<u64> {
    let file = p.fd.get(fd)?;
    if nbyte > IOSIZE_MAX {
        return Err(Errno::EINVAL);
    }
    let flags = *file.flags.lock();
    if !flags.write {
        return Err(Errno::EBADF);
    }
    match &file.kind {
        FileKind::Vnode(vp) => return vn_write(p, &file, *vp, buf, nbyte, flags.append),
        FileKind::Null | FileKind::Zero => return Ok(nbyte),
        _ => {}
    }
    // A pipe takes up to `PIPE_BUF` bytes at once, so chunks of that size
    // keep small writes atomic.
    let mut chunk = [0u8; crate::sys_pipe::PIPE_BUF];
    let mut done = 0;
    while done < nbyte {
        let n = usize_from_u64((nbyte - done).min(u64_from_usize(chunk.len())));
        if let Err(e) = p.vm().copyin(buf + done, &mut chunk[..n]) {
            // As FreeBSD: a partial transfer is a success.
            return if done > 0 { Ok(done) } else { Err(vm_errno(e)) };
        }
        let wrote = match &file.kind {
            FileKind::Console(_) => {
                tty::tty_write(&chunk[..n]);
                n
            }
            FileKind::Socket(sock) => {
                let extra = crate::uipc_socket::SendExtra {
                    rights: framework::mm::KVec::new(&crate::kern_malloc::M_TEMP),
                    to: None,
                    nosignal: false,
                    nonblock: flags.nonblock,
                };
                match crate::uipc_socket::kern_send(p, sock, &chunk[..n], extra) {
                    Ok(k) => k,
                    Err(e) => return if done > 0 { Ok(done) } else { Err(e) },
                }
            }
            FileKind::Pipe(pipe, _) => {
                let mut put = 0;
                while put < n {
                    match crate::sys_pipe::pipe_write(p, pipe, &chunk[put..n], flags.nonblock) {
                        Ok(k) => put += k,
                        Err(e) => {
                            let total = done + u64_from_usize(put);
                            return if total > 0 { Ok(total) } else { Err(e) };
                        }
                    }
                }
                put
            }
            _ => return Err(Errno::EBADF),
        };
        done += u64_from_usize(wrote);
    }
    Ok(done)
}

/// Writes to a file of a file system (`vn_write`): at the file's offset,
/// or at its end with `O_APPEND`; the offset moves past what was written.
fn vn_write(p: &Proc, file: &File, vp: crate::vnode::Vnode, buf: u64, nbyte: u64, append: bool) -> KResult<u64> {
    if vp.mp.rdonly {
        return Err(Errno::EROFS);
    }
    if vp.getattr()?.kind == VType::Dir {
        return Err(Errno::EISDIR);
    }
    crate::vnode_pager::vnode_pager_invalidate(&vp);
    let vm = p.vm();
    let mut off = file.offset.lock();
    if append {
        *off = vp.getattr()?.size;
    }
    let mut chunk = [0u8; 1024];
    let mut done = 0u64;
    while done < nbyte {
        let n = usize_from_u64((nbyte - done).min(u64_from_usize(chunk.len())));
        if let Err(e) = vm.copyin(buf + done, &mut chunk[..n]) {
            return if done > 0 { Ok(done) } else { Err(vm_errno(e)) };
        }
        let wrote = match vp.mp.ops.write(vp.ino, *off, &chunk[..n]) {
            Ok(w) => w,
            Err(e) => return if done > 0 { Ok(done) } else { Err(e) },
        };
        *off += u64_from_usize(wrote);
        done += u64_from_usize(wrote);
        if wrote < n {
            break;
        }
    }
    Ok(done)
}

/// Reads up to `nbyte` bytes from descriptor `fd` into user address `buf`
/// (`kern_readv`). Returns the bytes read; 0 at end of file.
pub fn kern_read(p: &Proc, fd: i32, buf: u64, nbyte: u64) -> KResult<u64> {
    let file = p.fd.get(fd)?;
    if nbyte > IOSIZE_MAX {
        return Err(Errno::EINVAL);
    }
    let flags = *file.flags.lock();
    if !flags.read {
        return Err(Errno::EBADF);
    }
    let vm = p.vm();
    let mut chunk = [0u8; 1024];
    let n = usize_from_u64(nbyte.min(u64_from_usize(chunk.len())));
    let vp = match &file.kind {
        FileKind::Console(_) => {
            let got = tty::tty_read(p, &mut chunk[..n])?;
            vm.copyout(buf, &chunk[..got]).map_err(vm_errno)?;
            return Ok(u64_from_usize(got));
        }
        FileKind::Pipe(pipe, _) => {
            let got = crate::sys_pipe::pipe_read(p, pipe, &mut chunk[..n], flags.nonblock)?;
            vm.copyout(buf, &chunk[..got]).map_err(vm_errno)?;
            return Ok(u64_from_usize(got));
        }
        FileKind::Socket(sock) => {
            let r = crate::uipc_socket::kern_recv(p, sock, &mut chunk[..n], false, flags.nonblock)?;
            vm.copyout(buf, &chunk[..r.len]).map_err(vm_errno)?;
            return Ok(u64_from_usize(r.len));
        }
        FileKind::ProcDesc(_) => return Err(Errno::EOPNOTSUPP),
        FileKind::Null => return Ok(0),
        FileKind::Zero => {
            vm.copyout(buf, &chunk[..n]).map_err(vm_errno)?;
            return Ok(u64_from_usize(n));
        }
        FileKind::Vnode(vp) => *vp,
    };
    if vp.getattr()?.kind == VType::Dir {
        return Err(Errno::EISDIR);
    }
    // `vn_read`: from the file's offset, which moves by what was read.
    let mut off = file.offset.lock();
    let mut done = 0u64;
    while done < nbyte {
        let n = usize_from_u64((nbyte - done).min(u64_from_usize(chunk.len())));
        let got = vp.read(*off, &mut chunk[..n])?;
        if got == 0 {
            break;
        }
        if let Err(e) = vm.copyout(buf + done, &chunk[..got]) {
            return if done > 0 { Ok(done) } else { Err(vm_errno(e)) };
        }
        *off += u64_from_usize(got);
        done += u64_from_usize(got);
    }
    Ok(done)
}

/// Closes descriptor `fd` (`kern_close`).
pub fn kern_close(p: &Proc, fd: i32) -> KResult<()> {
    p.fd.close(fd)
}

/// The status of descriptor `fd` (`kern_fstat`).
pub fn kern_fstat(p: &Proc, fd: i32) -> KResult<Stat> {
    let file = p.fd.get(fd)?;
    file_stat(&file)
}

/// Where `lseek` counts from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Whence {
    /// The start (`SEEK_SET`).
    Set,
    /// The current offset (`SEEK_CUR`).
    Cur,
    /// The end (`SEEK_END`).
    End,
}

/// Moves the offset of `fd` (`kern_lseek`); returns the new one.
/// `ESPIPE` on a terminal.
pub fn kern_lseek(p: &Proc, fd: i32, offset: i64, whence: Whence) -> KResult<u64> {
    let file = p.fd.get(fd)?;
    let vp = match &file.kind {
        FileKind::Vnode(vp) => *vp,
        // Devices without a position: the offset stays 0.
        FileKind::Null | FileKind::Zero => return Ok(0),
        _ => return Err(Errno::ESPIPE),
    };
    let mut off = file.offset.lock();
    let base = match whence {
        Whence::Set => 0,
        Whence::Cur => *off,
        Whence::End => vp.getattr()?.size,
    };
    let new = base.checked_add_signed(offset).ok_or(Errno::EINVAL)?;
    if i64::try_from(new).is_err() {
        return Err(Errno::EINVAL);
    }
    *off = new;
    Ok(new)
}

/// The terminal behind `fd`, or `ENOTTY`.
fn tty_of(file: &File) -> KResult<()> {
    match &file.kind {
        FileKind::Console(_) => Ok(()),
        _ => Err(Errno::ENOTTY),
    }
}

/// Bytes that can be read from `fd` now (`FIONREAD`).
pub fn kern_fionread(p: &Proc, fd: i32) -> KResult<u64> {
    let file = p.fd.get(fd)?;
    Ok(match &file.kind {
        FileKind::Console(_) => u64_from_usize(tty::tty_nread()),
        FileKind::Pipe(pipe, _) => u64_from_usize(pipe.len()),
        FileKind::Vnode(vp) => vp.getattr()?.size.saturating_sub(*file.offset.lock()),
        FileKind::Socket(s) => u64_from_usize(s.nread()),
        FileKind::Null | FileKind::Zero | FileKind::ProcDesc(_) => 0,
    })
}

/// What can be asked of a terminal (the terminal `ioctl`s, by name).
#[derive(Clone, Copy, Debug)]
pub enum TtyOp {
    /// Its settings (`TIOCGETA`).
    GetAttr,
    /// New settings; `drain` waits for output, `flush` discards input
    /// (`TIOCSETA`, `TIOCSETAW`, `TIOCSETAF`).
    SetAttr {
        /// The new settings.
        termios: Termios,
        /// Discard unread input.
        flush: bool,
    },
    /// Its size (`TIOCGWINSZ`).
    GetWinsize,
    /// A new size (`TIOCSWINSZ`).
    SetWinsize(Winsize),
    /// Become the caller's controlling terminal (`TIOCSCTTY`).
    SetCtty,
    /// Stop being it (`TIOCNOTTY`).
    NotCtty,
    /// The foreground process group (`TIOCGPGRP`).
    GetPgrp,
    /// A new foreground process group (`TIOCSPGRP`).
    SetPgrp(u32),
    /// The session (`TIOCGSID`).
    GetSid,
    /// Bytes ready to read (`FIONREAD`).
    NRead,
}

/// The answer to a [`TtyOp`].
#[derive(Clone, Copy, Debug)]
pub enum TtyReply {
    /// Done; nothing to return.
    Done,
    /// The settings.
    Attr(Termios),
    /// The size.
    Winsize(Winsize),
    /// A process group, a session, or a count.
    Id(u32),
}

/// A terminal operation on descriptor `fd` (`kern_ioctl` on a tty);
/// `ENOTTY` if it is not a terminal.
pub fn kern_tty_ioctl(p: &Proc, fd: i32, op: TtyOp) -> KResult<TtyReply> {
    let file = p.fd.get(fd)?;
    tty_of(&file)?;
    Ok(match op {
        TtyOp::GetAttr => TtyReply::Attr(tty::tty_getattr()),
        TtyOp::SetAttr { termios, flush } => {
            tty::tty_setattr(&termios, flush);
            TtyReply::Done
        }
        TtyOp::GetWinsize => TtyReply::Winsize(tty::tty_getwinsize()),
        TtyOp::SetWinsize(w) => {
            tty::tty_setwinsize(&w);
            TtyReply::Done
        }
        TtyOp::SetCtty => {
            let dev = match file.kind {
                FileKind::Console(d) => d,
                _ => crate::devfs::CONSOLE_DEV,
            };
            tty::tty_setctty(p, dev)?;
            TtyReply::Done
        }
        TtyOp::NotCtty => {
            if !tty::tty_is_ctty(p.sid()) {
                return Err(Errno::ENOTTY);
            }
            if p.sid() == p.pid {
                tty::tty_release(p.sid());
            }
            TtyReply::Done
        }
        TtyOp::GetPgrp => TtyReply::Id(tty::tty_getpgrp(p)?),
        TtyOp::SetPgrp(g) => {
            tty::tty_setpgrp(p, g)?;
            TtyReply::Done
        }
        TtyOp::GetSid => TtyReply::Id(tty::tty_getsid(p)?),
        TtyOp::NRead => TtyReply::Id(u32::try_from(tty::tty_nread()).unwrap_or(u32::MAX)),
    })
}

/// Checks `fd` is open, for `ioctl`s that only set descriptor flags
/// (`FIOCLEX`, `FIONBIO`, ...) and have no effect yet.
pub fn kern_fd_valid(p: &Proc, fd: i32) -> KResult<()> {
    p.fd.get(fd).map(drop)
}

/// Reads from `fd` at `offset`, leaving its offset alone (`kern_pread`):
/// only a file of a file system has positions (`ESPIPE` otherwise).
pub fn kern_pread(p: &Proc, fd: i32, buf: u64, nbyte: u64, offset: u64) -> KResult<u64> {
    let file = p.fd.get(fd)?;
    if !file.flags.lock().read {
        return Err(Errno::EBADF);
    }
    let vp = file.kind.vnode().ok_or(Errno::ESPIPE)?;
    if vp.getattr()?.kind == VType::Dir {
        return Err(Errno::EISDIR);
    }
    if i64::try_from(offset).is_err() || nbyte > IOSIZE_MAX {
        return Err(Errno::EINVAL);
    }
    let vm = p.vm();
    let mut chunk = [0u8; 1024];
    let mut done = 0u64;
    while done < nbyte {
        let n = usize_from_u64((nbyte - done).min(u64_from_usize(chunk.len())));
        let got = vp.read(offset + done, &mut chunk[..n])?;
        if got == 0 {
            break;
        }
        if let Err(e) = vm.copyout(buf + done, &chunk[..got]) {
            return if done > 0 { Ok(done) } else { Err(vm_errno(e)) };
        }
        done += u64_from_usize(got);
    }
    Ok(done)
}

/// Writes to `fd` at `offset`, leaving its offset alone (`kern_pwrite`).
pub fn kern_pwrite(p: &Proc, fd: i32, buf: u64, nbyte: u64, offset: u64) -> KResult<u64> {
    let file = p.fd.get(fd)?;
    if !file.flags.lock().write {
        return Err(Errno::EBADF);
    }
    let vp = file.kind.vnode().ok_or(Errno::ESPIPE)?;
    if vp.mp.rdonly {
        return Err(Errno::EROFS);
    }
    crate::vnode_pager::vnode_pager_invalidate(&vp);
    let vm = p.vm();
    let mut chunk = [0u8; 1024];
    let mut done = 0u64;
    while done < nbyte {
        let n = usize_from_u64((nbyte - done).min(u64_from_usize(chunk.len())));
        if let Err(e) = vm.copyin(buf + done, &mut chunk[..n]) {
            return if done > 0 { Ok(done) } else { Err(vm_errno(e)) };
        }
        let wrote = vp.mp.ops.write(vp.ino, offset + done, &chunk[..n])?;
        done += u64_from_usize(wrote);
        if wrote < n {
            break;
        }
    }
    Ok(done)
}

/// What `poll` asks of, and answers for, one descriptor.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools, reason = "the independent POLL* bits")]
pub struct PollFd {
    /// The descriptor (negative: ignored).
    pub fd: i32,
    /// Wait until it can be read without sleeping.
    pub read: bool,
    /// Wait until it can be written without sleeping.
    pub write: bool,
    /// Answer: it can be read.
    pub readable: bool,
    /// Answer: it can be written.
    pub writable: bool,
    /// Answer: the other end is gone (`POLLHUP`).
    pub hup: bool,
    /// Answer: not an open descriptor (`POLLNVAL`).
    pub invalid: bool,
}

/// Whether `file` can be read and written now, and whether its other
/// end is gone (`fo_poll`).
fn file_ready(file: &File) -> (bool, bool, bool) {
    match &file.kind {
        FileKind::Console(_) => (tty::tty_readable(), true, false),
        FileKind::Pipe(pipe, crate::sys_pipe::PipeEnd::Read) => {
            let hup = pipe.writers_gone();
            (!pipe.is_empty() || hup, false, hup)
        }
        FileKind::Pipe(pipe, crate::sys_pipe::PipeEnd::Write) => {
            let hup = pipe.readers_gone();
            (false, pipe.has_room() || hup, hup)
        }
        FileKind::Socket(s) => {
            let hup = s.hung_up();
            (s.readable() || hup, s.writable(), hup)
        }
        FileKind::ProcDesc(pd) => {
            let gone = pd.exited();
            (gone, false, gone)
        }
        FileKind::Vnode(_) | FileKind::Null | FileKind::Zero => (true, true, false),
    }
}

/// Waits until one of `fds` is ready, `timeout_ns` passes (`None`:
/// forever), or a signal comes (`kern_poll`); returns how many are ready
/// (their answers filled in). Checked on every clock tick: a wakeup per
/// file (`selwakeup`) is for later.
pub fn kern_poll(p: &Proc, fds: &mut [PollFd], timeout_ns: Option<u64>) -> KResult<usize> {
    let tick = 1_000_000_000 / u64::from(crate::kern_clock::hz().max(1));
    let mut left = timeout_ns;
    loop {
        let mut n = 0;
        for f in fds.iter_mut() {
            f.readable = false;
            f.writable = false;
            f.hup = false;
            f.invalid = false;
            if f.fd < 0 {
                continue;
            }
            match p.fd.get(f.fd) {
                Ok(file) => {
                    let (r, w, hup) = file_ready(&file);
                    f.readable = f.read && r;
                    f.writable = f.write && w;
                    f.hup = hup;
                }
                Err(_) => f.invalid = true,
            }
            if f.readable || f.writable || f.hup || f.invalid {
                n += 1;
            }
        }
        if n > 0 || left == Some(0) {
            return Ok(n);
        }
        let step = left.map_or(tick, |l| l.min(tick));
        crate::kern_synch::kern_nanosleep_sig(p, step)?;
        left = left.map(|l| l.saturating_sub(step));
    }
}

/// The name of the device `fd` is open on (`FIODGNAME`, for `ttyname`):
/// `EINVAL` for anything but a device.
pub fn kern_fiodgname(p: &Proc, fd: i32) -> KResult<&'static [u8]> {
    let file = p.fd.get(fd)?;
    let rdev = match &file.kind {
        FileKind::Console(d) => *d,
        FileKind::Null => 4,
        FileKind::Zero => 6,
        _ => return Err(Errno::EINVAL),
    };
    crate::devfs::devname(rdev).ok_or(Errno::EINVAL)
}
