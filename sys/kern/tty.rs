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

//! The console terminal (FreeBSD's `kern/tty.c` and the line discipline
//! of `kern/tty_ttydisc.c`).
//!
//! In canonical mode (`ICANON`) input is edited a line at a time: `VERASE`
//! and `VERASE2` remove a character, `VWERASE` a word, `VKILL` the line,
//! `VEOF` ends a read; a read returns once a line is complete. Otherwise
//! bytes are returned as they come (`VMIN` 1). Echo follows `ECHO`,
//! `ECHOE`, `ECHOKE`, `ECHOCTL` and `ECHONL`.
//!
//! Input comes from the serial line, polled on every clock tick (there is
//! no interrupt controller driver yet), so typing works on the console of
//! bhyve and QEMU.
//!
//! TODO(tty): process groups and job control (`ISIG` characters only echo
//! and discard the line), the keyboard on the screen, flow control,
//! `TIOCSTI`, more than one terminal.

use core::sync::atomic::{AtomicBool, Ordering};

use framework::sync::{MtxSpin, WaitQueue};
use sys::termios::{Cc, TFlag, Termios, Winsize};

use crate::kern_cons;

/// Bytes the input queue holds (FreeBSD's `TTYINQ_DATASIZE` is per block;
/// one fixed queue is enough for a console).
const INQ_SIZE: usize = 4096;
/// Longest line being edited.
const LINE_MAX: usize = 1024;

struct Tty {
    termios: Termios,
    winsize: Winsize,
    /// Input ready to be read.
    inq: [u8; INQ_SIZE],
    head: usize,
    len: usize,
    /// The line being edited (canonical mode).
    line: [u8; LINE_MAX],
    line_len: usize,
    /// `VEOF` was typed on an empty line: the next read returns 0.
    eof: bool,
    /// `VLNEXT` was typed: the next character is taken literally.
    lnext: bool,
}

static TTY: MtxSpin<Tty> = MtxSpin::new(Tty {
    termios: Termios::DEFAULT,
    winsize: Winsize { row: 24, col: 80, xpixel: 0, ypixel: 0 },
    inq: [0; INQ_SIZE],
    head: 0,
    len: 0,
    line: [0; LINE_MAX],
    line_len: 0,
    eof: false,
    lnext: false,
});

/// Readers waiting for input.
static READERS: WaitQueue = WaitQueue::new();
/// There is something for a reader (input or end of file).
static READY: AtomicBool = AtomicBool::new(false);

/// Sets the console's size (from the video console, if it has one).
pub fn tty_set_size(cols: u16, rows: u16) {
    let mut t = TTY.lock();
    t.winsize.col = cols;
    t.winsize.row = rows;
}

fn echo(b: u8) {
    kern_cons::cnputc_attr(b, vt::Attr::Normal);
}

impl Tty {
    fn push_input(&mut self, b: u8) -> bool {
        if self.len == INQ_SIZE {
            return false;
        }
        let tail = (self.head + self.len) % INQ_SIZE;
        self.inq[tail] = b;
        self.len += 1;
        true
    }

    fn has(&self, f: TFlag) -> bool {
        self.termios.flags.has(f)
    }

    /// Echoes a typed character (`ttydisc_echo`): a control character as
    /// `^X` with `ECHOCTL`.
    fn echo_char(&self, c: u8) {
        if !self.has(TFlag::ECHO) {
            return;
        }
        if self.has(TFlag::ECHOCTL) && (c < 0x20 && c != b'\n' && c != b'\t' || c == 0x7f) {
            echo(b'^');
            echo(if c == 0x7f { b'?' } else { c + b'@' });
        } else {
            echo(c);
        }
    }

    /// Erases the last character of the line on the screen too.
    fn erase_one(&mut self) {
        // A UTF-8 character is erased whole (IUTF8).
        while self.line_len > 0 {
            self.line_len -= 1;
            let c = self.line[self.line_len];
            if !self.has(TFlag::IUTF8) || c & 0xc0 != 0x80 {
                if self.has(TFlag::ECHO) && self.has(TFlag::ECHOE) {
                    let width = if c < 0x20 && self.has(TFlag::ECHOCTL) { 2 } else { 1 };
                    for _ in 0..width {
                        echo(0x08);
                        echo(b' ');
                        echo(0x08);
                    }
                }
                break;
            }
        }
    }

    /// Hands the edited line to readers.
    fn commit_line(&mut self) {
        for i in 0..self.line_len {
            let b = self.line[i];
            if !self.push_input(b) {
                break;
            }
        }
        self.line_len = 0;
        READY.store(true, Ordering::Release);
    }

    /// One input byte through the line discipline (`ttydisc_rint`).
    fn rint(&mut self, mut c: u8) {
        if self.lnext {
            self.lnext = false;
            return self.store(c);
        }
        if self.has(TFlag::ISTRIP) {
            c &= 0x7f;
        }
        match c {
            b'\r' if self.has(TFlag::IGNCR) => return,
            b'\r' if self.has(TFlag::ICRNL) => c = b'\n',
            b'\n' if self.has(TFlag::INLCR) => c = b'\r',
            _ => {}
        }
        let is = |cc: Cc| self.termios.cc(cc) == c && c != sys::termios::VDISABLE;
        if self.has(TFlag::IEXTEN) && is(Cc::VLNEXT) {
            self.lnext = true;
            return;
        }
        if self.has(TFlag::ISIG) && (is(Cc::VINTR) || is(Cc::VQUIT) || is(Cc::VSUSP)) {
            // No process groups yet: show it and drop the line.
            self.echo_char(c);
            if !self.has(TFlag::NOFLSH) {
                self.line_len = 0;
            }
            echo(b'\n');
            return;
        }
        if !self.has(TFlag::ICANON) {
            self.echo_char(c);
            self.push_input(c);
            READY.store(true, Ordering::Release);
            return;
        }
        if is(Cc::VERASE) || is(Cc::VERASE2) {
            return self.erase_one();
        }
        if self.has(TFlag::IEXTEN) && is(Cc::VWERASE) {
            while self.line_len > 0 && self.line[self.line_len - 1] == b' ' {
                self.erase_one();
            }
            while self.line_len > 0 && self.line[self.line_len - 1] != b' ' {
                self.erase_one();
            }
            return;
        }
        if is(Cc::VKILL) {
            if self.has(TFlag::ECHOKE) {
                while self.line_len > 0 {
                    self.erase_one();
                }
            } else {
                self.echo_char(c);
                self.line_len = 0;
                if self.has(TFlag::ECHOK) {
                    echo(b'\n');
                }
            }
            return;
        }
        if is(Cc::VEOF) {
            if self.line_len == 0 {
                self.eof = true;
                READY.store(true, Ordering::Release);
            } else {
                self.commit_line();
            }
            return;
        }
        if c == b'\n' || is(Cc::VEOL) || is(Cc::VEOL2) {
            if self.has(TFlag::ECHO) || (c == b'\n' && self.has(TFlag::ECHONL)) {
                echo(c);
            }
            self.store_raw(c);
            return self.commit_line();
        }
        self.echo_char(c);
        self.store(c);
    }

    /// Adds a byte to the line being edited (or to the input queue).
    fn store(&mut self, c: u8) {
        if !self.has(TFlag::ICANON) {
            self.push_input(c);
            READY.store(true, Ordering::Release);
            return;
        }
        self.store_raw(c);
    }

    fn store_raw(&mut self, c: u8) {
        if self.line_len < LINE_MAX {
            self.line[self.line_len] = c;
            self.line_len += 1;
        } else if self.has(TFlag::IMAXBEL) {
            echo(0x07);
        }
    }

    /// Takes up to `out.len()` bytes of input: in canonical mode, at most
    /// one line.
    fn take(&mut self, out: &mut [u8]) -> usize {
        let canon = self.has(TFlag::ICANON);
        let mut n = 0;
        while n < out.len() && self.len > 0 {
            let b = self.inq[self.head];
            self.head = (self.head + 1) % INQ_SIZE;
            self.len -= 1;
            out[n] = b;
            n += 1;
            if canon && b == b'\n' {
                break;
            }
        }
        if self.len == 0 && !self.eof {
            READY.store(false, Ordering::Release);
        }
        n
    }
}

/// Polls the console for typed bytes and feeds them to the terminal. Called
/// on every clock tick, in interrupt context.
pub fn tty_poll() {
    let mut got = false;
    while let Some(c) = kern_cons::cncheckc() {
        TTY.lock().rint(c);
        got = true;
    }
    if got && READY.load(Ordering::Acquire) {
        READERS.wake_all();
    }
}

/// Reads input into `out` (`ttydisc_read`): sleeps until there is some, or
/// end of file. Returns the bytes read; 0 at end of file.
pub fn tty_read(out: &mut [u8]) -> usize {
    loop {
        READERS.wait_until(|| READY.load(Ordering::Acquire));
        let mut t = TTY.lock();
        if t.len > 0 {
            return t.take(out);
        }
        if t.eof {
            t.eof = false;
            READY.store(false, Ordering::Release);
            return 0;
        }
        READY.store(false, Ordering::Release);
    }
}

/// Writes program output (`ttydisc_write`).
pub fn tty_write(buf: &[u8]) {
    crate::subr_prf::cnwrite(buf);
}

/// The terminal's settings (`TIOCGETA`).
pub fn tty_getattr() -> Termios {
    TTY.lock().termios
}

/// Changes the terminal's settings (`TIOCSETA`); `flush` discards
/// unread input (`TIOCSETAF`).
pub fn tty_setattr(t: &Termios, flush: bool) {
    let mut tty = TTY.lock();
    let was_canon = tty.has(TFlag::ICANON);
    tty.termios = *t;
    if flush {
        tty.len = 0;
        tty.line_len = 0;
    } else if was_canon && !tty.has(TFlag::ICANON) {
        // Leaving canonical mode: what was being edited becomes input.
        tty.commit_line();
    }
}

/// The terminal's size (`TIOCGWINSZ`).
pub fn tty_getwinsize() -> Winsize {
    TTY.lock().winsize
}

/// Sets the terminal's size (`TIOCSWINSZ`).
pub fn tty_setwinsize(w: &Winsize) {
    TTY.lock().winsize = *w;
}
