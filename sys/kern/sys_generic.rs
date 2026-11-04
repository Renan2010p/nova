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

/// Largest single transfer (`IOSIZE_MAX`).
const IOSIZE_MAX: u64 = i64::MAX.unsigned_abs();

/// Writes `nbyte` bytes from user address `buf` to descriptor `fd`
/// (`kern_writev`). Returns the bytes written.
pub fn kern_write(p: &Proc, fd: i32, buf: u64, nbyte: u64) -> KResult<u64> {
    let file = p.fd.get(fd)?;
    if nbyte > IOSIZE_MAX {
        return Err(Errno::EINVAL);
    }
    let mut chunk = [0u8; 256];
    let mut done = 0;
    while done < nbyte {
        let n = usize_from_u64((nbyte - done).min(u64_from_usize(chunk.len())));
        if let Err(e) = p.vm.copyin(buf + done, &mut chunk[..n]) {
            // As FreeBSD: a partial transfer is a success.
            return if done > 0 { Ok(done) } else { Err(vm_errno(e)) };
        }
        match file.kind {
            FileKind::Console => tty::tty_write(&chunk[..n]),
        }
        done += u64_from_usize(n);
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
    let mut chunk = [0u8; 1024];
    let n = usize_from_u64(nbyte.min(u64_from_usize(chunk.len())));
    let got = match file.kind {
        FileKind::Console => tty::tty_read(&mut chunk[..n]),
    };
    p.vm.copyout(buf, &chunk[..got]).map_err(vm_errno)?;
    Ok(u64_from_usize(got))
}

/// Closes descriptor `fd` (`kern_close`).
pub fn kern_close(p: &Proc, fd: i32) -> KResult<()> {
    p.fd.close(fd)
}

/// The status of descriptor `fd` (`kern_fstat`).
pub fn kern_fstat(p: &Proc, fd: i32) -> KResult<Stat> {
    let file = p.fd.get(fd)?;
    Ok(file_stat(&file))
}

/// The terminal behind `fd`, or `ENOTTY`.
fn tty_of(file: &File) -> KResult<()> {
    match file.kind {
        FileKind::Console => Ok(()),
    }
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
    })
}

/// Checks `fd` is open, for `ioctl`s that only set descriptor flags
/// (`FIOCLEX`, `FIONBIO`, ...) and have no effect yet.
pub fn kern_fd_valid(p: &Proc, fd: i32) -> KResult<()> {
    p.fd.get(fd).map(drop)
}
