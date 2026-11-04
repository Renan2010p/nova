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

//! Terminal settings, by name (POSIX `termios`, `winsize`).
//!
//! As with signals and errors, the kernel names the settings and the ABIs
//! give them their bits: a flag here is a name, its value in a program's
//! `struct termios` is the ABI's business (`sys/compat/<abi>`).

/// A terminal flag (from `c_iflag`, `c_oflag`, `c_cflag` or `c_lflag`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(clippy::upper_case_acronyms, non_camel_case_types, reason = "the POSIX names")]
pub enum TFlag {
    /// Ignore a break.
    IGNBRK,
    /// A break interrupts.
    BRKINT,
    /// Ignore parity errors.
    IGNPAR,
    /// Mark parity errors.
    PARMRK,
    /// Check parity.
    INPCK,
    /// Strip the eighth bit.
    ISTRIP,
    /// Map NL to CR on input.
    INLCR,
    /// Ignore CR on input.
    IGNCR,
    /// Map CR to NL on input.
    ICRNL,
    /// Output flow control.
    IXON,
    /// Input flow control.
    IXOFF,
    /// Any character restarts output.
    IXANY,
    /// Ring the bell when the input queue is full.
    IMAXBEL,
    /// Input is UTF-8 (erase removes whole characters).
    IUTF8,
    /// Process output.
    OPOST,
    /// Map NL to CR-NL on output.
    ONLCR,
    /// Expand tabs to spaces.
    OXTABS,
    /// Discard EOT on output.
    ONOEOT,
    /// Map CR to NL on output.
    OCRNL,
    /// No CR output at column 0.
    ONOCR,
    /// NL does a CR.
    ONLRET,
    /// Two stop bits.
    CSTOPB,
    /// Enable the receiver.
    CREAD,
    /// Parity.
    PARENB,
    /// Odd parity.
    PARODD,
    /// Hang up on last close.
    HUPCL,
    /// Ignore modem status lines.
    CLOCAL,
    /// CTS flow control of output.
    CCTS_OFLOW,
    /// RTS flow control of input.
    CRTS_IFLOW,
    /// Erase the line visually on kill.
    ECHOKE,
    /// Erase characters visually.
    ECHOE,
    /// Echo NL after kill.
    ECHOK,
    /// Echo input.
    ECHO,
    /// Echo NL even without ECHO.
    ECHONL,
    /// Hardcopy erase.
    ECHOPRT,
    /// Echo control characters as ^X.
    ECHOCTL,
    /// Generate signals from VINTR, VQUIT, VSUSP.
    ISIG,
    /// Canonical input: line editing, read a line at a time.
    ICANON,
    /// Alternate word erase.
    ALTWERASE,
    /// Extended functions (VLNEXT, VDISCARD, ...).
    IEXTEN,
    /// External processing.
    EXTPROC,
    /// Stop background writers.
    TOSTOP,
    /// Output being flushed.
    FLUSHO,
    /// No kernel status on VSTATUS.
    NOKERNINFO,
    /// Retype pending input.
    PENDIN,
    /// No flush after interrupt.
    NOFLSH,
}

impl TFlag {
    /// The flag's position in a [`TFlags`].
    pub const fn index(self) -> u32 {
        match self {
            TFlag::IGNBRK => 0,
            TFlag::BRKINT => 1,
            TFlag::IGNPAR => 2,
            TFlag::PARMRK => 3,
            TFlag::INPCK => 4,
            TFlag::ISTRIP => 5,
            TFlag::INLCR => 6,
            TFlag::IGNCR => 7,
            TFlag::ICRNL => 8,
            TFlag::IXON => 9,
            TFlag::IXOFF => 10,
            TFlag::IXANY => 11,
            TFlag::IMAXBEL => 12,
            TFlag::IUTF8 => 13,
            TFlag::OPOST => 14,
            TFlag::ONLCR => 15,
            TFlag::OXTABS => 16,
            TFlag::ONOEOT => 17,
            TFlag::OCRNL => 18,
            TFlag::ONOCR => 19,
            TFlag::ONLRET => 20,
            TFlag::CSTOPB => 21,
            TFlag::CREAD => 22,
            TFlag::PARENB => 23,
            TFlag::PARODD => 24,
            TFlag::HUPCL => 25,
            TFlag::CLOCAL => 26,
            TFlag::CCTS_OFLOW => 27,
            TFlag::CRTS_IFLOW => 28,
            TFlag::ECHOKE => 29,
            TFlag::ECHOE => 30,
            TFlag::ECHOK => 31,
            TFlag::ECHO => 32,
            TFlag::ECHONL => 33,
            TFlag::ECHOPRT => 34,
            TFlag::ECHOCTL => 35,
            TFlag::ISIG => 36,
            TFlag::ICANON => 37,
            TFlag::ALTWERASE => 38,
            TFlag::IEXTEN => 39,
            TFlag::EXTPROC => 40,
            TFlag::TOSTOP => 41,
            TFlag::FLUSHO => 42,
            TFlag::NOKERNINFO => 43,
            TFlag::PENDIN => 44,
            TFlag::NOFLSH => 45,
        }
    }
}

/// A set of terminal flags.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TFlags(u64);

impl TFlags {
    /// No flag.
    pub const EMPTY: TFlags = TFlags(0);

    const fn bit(f: TFlag) -> u64 {
        1 << f.index()
    }

    /// The set of `flags`.
    pub const fn of(flags: &[TFlag]) -> TFlags {
        let mut v = 0;
        let mut i = 0;
        while i < flags.len() {
            v |= Self::bit(flags[i]);
            i += 1;
        }
        TFlags(v)
    }

    /// True if `f` is set.
    pub const fn has(self, f: TFlag) -> bool {
        self.0 & Self::bit(f) != 0
    }

    /// Sets or clears `f`.
    pub fn set(&mut self, f: TFlag, on: bool) {
        if on {
            self.0 |= Self::bit(f);
        } else {
            self.0 &= !Self::bit(f);
        }
    }
}

/// A special character (an index of `c_cc`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(clippy::upper_case_acronyms, reason = "the POSIX names")]
pub enum Cc {
    /// End of file (^D).
    VEOF,
    /// End of line.
    VEOL,
    /// Second end of line.
    VEOL2,
    /// Erase a character (DEL).
    VERASE,
    /// Erase a word (^W).
    VWERASE,
    /// Erase the line (^U).
    VKILL,
    /// Reprint the line (^R).
    VREPRINT,
    /// Second erase (^H).
    VERASE2,
    /// Interrupt (^C).
    VINTR,
    /// Quit (^\\).
    VQUIT,
    /// Suspend (^Z).
    VSUSP,
    /// Delayed suspend (^Y).
    VDSUSP,
    /// Start output (^Q).
    VSTART,
    /// Stop output (^S).
    VSTOP,
    /// Take the next character literally (^V).
    VLNEXT,
    /// Discard output (^O).
    VDISCARD,
    /// Minimum bytes for a non-canonical read.
    VMIN,
    /// Timeout of a non-canonical read, in tenths of a second.
    VTIME,
    /// Status request (^T).
    VSTATUS,
}

impl Cc {
    /// The character's index in [`Termios::cc`].
    pub const fn index(self) -> usize {
        match self {
            Cc::VEOF => 0,
            Cc::VEOL => 1,
            Cc::VEOL2 => 2,
            Cc::VERASE => 3,
            Cc::VWERASE => 4,
            Cc::VKILL => 5,
            Cc::VREPRINT => 6,
            Cc::VERASE2 => 7,
            Cc::VINTR => 8,
            Cc::VQUIT => 9,
            Cc::VSUSP => 10,
            Cc::VDSUSP => 11,
            Cc::VSTART => 12,
            Cc::VSTOP => 13,
            Cc::VLNEXT => 14,
            Cc::VDISCARD => 15,
            Cc::VMIN => 16,
            Cc::VTIME => 17,
            Cc::VSTATUS => 18,
        }
    }
}

/// Number of special characters.
pub const NCC: usize = 19;

/// A disabled special character (`_POSIX_VDISABLE`).
pub const VDISABLE: u8 = 0xff;

/// A terminal's settings (`struct termios`, by name).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Termios {
    /// The flags.
    pub flags: TFlags,
    /// Bits per character, 5 to 8 (`CSIZE`).
    pub csize: u8,
    /// The special characters, by `Cc`.
    pub cc: [u8; NCC],
    /// Input speed, in bits per second.
    pub ispeed: u32,
    /// Output speed, in bits per second.
    pub ospeed: u32,
}

const fn ctrl(c: u8) -> u8 {
    c & 0x1f
}

impl Termios {
    /// The settings a terminal starts with (FreeBSD's `ttydefaults.h`).
    pub const DEFAULT: Termios = {
        use TFlag::*;
        let mut cc = [VDISABLE; NCC];
        cc[Cc::VEOF.index()] = ctrl(b'D');
        cc[Cc::VERASE.index()] = 0x7f;
        cc[Cc::VWERASE.index()] = ctrl(b'W');
        cc[Cc::VKILL.index()] = ctrl(b'U');
        cc[Cc::VREPRINT.index()] = ctrl(b'R');
        cc[Cc::VERASE2.index()] = ctrl(b'H');
        cc[Cc::VINTR.index()] = ctrl(b'C');
        cc[Cc::VQUIT.index()] = ctrl(b'\\');
        cc[Cc::VSUSP.index()] = ctrl(b'Z');
        cc[Cc::VDSUSP.index()] = ctrl(b'Y');
        cc[Cc::VSTART.index()] = ctrl(b'Q');
        cc[Cc::VSTOP.index()] = ctrl(b'S');
        cc[Cc::VLNEXT.index()] = ctrl(b'V');
        cc[Cc::VDISCARD.index()] = ctrl(b'O');
        cc[Cc::VMIN.index()] = 1;
        cc[Cc::VTIME.index()] = 0;
        cc[Cc::VSTATUS.index()] = ctrl(b'T');
        Termios {
            flags: TFlags::of(&[
                BRKINT, ICRNL, IMAXBEL, IXON, IXANY, IUTF8, OPOST, ONLCR, CREAD, HUPCL, ICANON, ISIG,
                IEXTEN, ECHO, ECHOE, ECHOKE, ECHOCTL,
            ]),
            csize: 8,
            cc,
            ispeed: 9600,
            ospeed: 9600,
        }
    };

    /// The special character `c`.
    pub fn cc(&self, c: Cc) -> u8 {
        self.cc.get(c.index()).copied().unwrap_or(VDISABLE)
    }
}

/// A terminal's size (`struct winsize`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Winsize {
    /// Rows, in characters.
    pub row: u16,
    /// Columns, in characters.
    pub col: u16,
    /// Width, in pixels.
    pub xpixel: u16,
    /// Height, in pixels.
    pub ypixel: u16,
}
