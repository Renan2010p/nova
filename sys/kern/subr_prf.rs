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

//! Kernel `printf` (FreeBSD's `kern/subr_prf.c`), on top of Rust's
//! `core::fmt` instead of a C format string. What the kernel prints is
//! kept in the message buffer too (`msgbuf`, `kern/subr_msgbuf.c`), which
//! `dmesg` reads.

use core::fmt::{self, Write};

use framework::sync::MtxSpin;

use crate::kern_cons::cnputs;

/// Serializes whole messages so lines from different CPUs don't mix.
static PRF_LOCK: MtxSpin<()> = MtxSpin::new(());

/// Size of the message buffer (FreeBSD amd64's `MSGBUF_SIZE`).
pub const MSGBUF_SIZE: usize = 0x18000;

/// The message buffer: the last [`MSGBUF_SIZE`] bytes printed.
struct MsgBuf {
    buf: [u8; MSGBUF_SIZE],
    /// Where the next byte goes.
    wptr: usize,
    /// Bytes held (at most the size: then it has wrapped).
    len: usize,
}

static MSGBUF: MtxSpin<MsgBuf> = MtxSpin::new(MsgBuf { buf: [0; MSGBUF_SIZE], wptr: 0, len: 0 });

impl MsgBuf {
    fn add(&mut self, s: &[u8]) {
        for &b in s {
            if let Some(slot) = self.buf.get_mut(self.wptr) {
                *slot = b;
            }
            self.wptr = (self.wptr + 1) % MSGBUF_SIZE;
            self.len = (self.len + 1).min(MSGBUF_SIZE);
        }
    }
}

/// Adds to the message buffer (`msgbuf_addstr`). From `panic`, which may
/// have interrupted its holder, only if the lock is free.
fn msgbuf_add(s: &str, panicking: bool) {
    let guard = if panicking { MSGBUF.try_lock() } else { Some(MSGBUF.lock()) };
    if let Some(mut m) = guard {
        m.add(s.as_bytes());
    }
}

/// Bytes in the message buffer.
pub fn msgbuf_len() -> usize {
    MSGBUF.lock().len
}

/// Copies the message buffer from `off` bytes after its oldest into
/// `out` (`msgbuf_peekbytes`); returns the bytes copied. The lock is held
/// only while copying, so callers copy to user memory after.
pub fn msgbuf_read(off: usize, out: &mut [u8]) -> usize {
    let m = MSGBUF.lock();
    let start = (m.wptr + MSGBUF_SIZE - m.len) % MSGBUF_SIZE;
    let n = out.len().min(m.len.saturating_sub(off));
    for (i, b) in out.iter_mut().take(n).enumerate() {
        *b = m.buf.get((start + off + i) % MSGBUF_SIZE).copied().unwrap_or(0);
    }
    n
}

/// The console as a `fmt::Write`, for code that formats piecewise (each
/// piece is printed whole, but pieces from different CPUs may interleave).
pub struct Cons;

impl Write for Cons {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let _guard = PRF_LOCK.lock();
        msgbuf_add(s, false);
        cnputs(s);
        Ok(())
    }
}

/// The console and the message buffer, with the console lock held.
struct ConsLogged;

impl Write for ConsLogged {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        msgbuf_add(s, false);
        cnputs(s);
        Ok(())
    }
}

/// The console without the lock. Only for `panic`, which may have
/// interrupted a CPU that holds it.
pub(crate) struct ConsUnlocked;

impl Write for ConsUnlocked {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        msgbuf_add(s, true);
        cnputs(s);
        Ok(())
    }
}

/// Back-end of [`printf!`](crate::printf). Use the macro.
#[doc(hidden)]
pub fn vprintf(args: fmt::Arguments<'_>) {
    let _guard = PRF_LOCK.lock();
    let _ = ConsLogged.write_fmt(args);
}

/// Prints without taking the console lock. Only for `panic`.
pub(crate) fn vprintf_unlocked(args: fmt::Arguments<'_>) {
    let _ = ConsUnlocked.write_fmt(args);
}

/// Writes raw bytes to the console as one message: what a program writes
/// to the console device.
pub fn cnwrite(buf: &[u8]) {
    let _guard = PRF_LOCK.lock();
    for &b in buf {
        crate::kern_cons::cnputc_attr(b, vt::Attr::Normal);
    }
}

/// Logs a message (`log(9)`). With no `syslogd` reading the log, which is
/// always the case for now, it goes to the console, as in FreeBSD.
#[macro_export]
macro_rules! log {
    ($($arg:tt)*) => {
        $crate::subr_prf::vprintf(::core::format_args!($($arg)*))
    };
}

/// Prints to the kernel console, like `format!`.
#[macro_export]
macro_rules! printf {
    ($($arg:tt)*) => {
        $crate::subr_prf::vprintf(::core::format_args!($($arg)*))
    };
}
