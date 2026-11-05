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

//! FreeBSD's structures (`sys/sys/*.h`) and the kernel's, both ways:
//! `struct termios`, `struct winsize` and `struct stat`.
//!
//! Sizes and offsets are FreeBSD 15.1's on amd64 and riscv64 (LP64), as
//! `tools/abidump` prints them on FreeBSD; the `const` checks below fail
//! the build if a layout here disagrees.

use kern::kern_descrip::{FileType, Stat};
use sys::bytes::read_u32;
use sys::termios::{Cc, NCC, TFlag, TFlags, Termios, VDISABLE, Winsize};

/// `sizeof(struct termios)`.
pub(crate) const TERMIOS_SIZE: usize = 44;
/// `NCCS`, and where `c_cc` and the speeds are.
const NCCS: usize = 20;
const CC_OFF: usize = 16;
const ISPEED_OFF: usize = 36;
const OSPEED_OFF: usize = 40;
const _: () = assert!(CC_OFF + NCCS == ISPEED_OFF && OSPEED_OFF + 4 == TERMIOS_SIZE);

/// `sizeof(struct winsize)`.
pub(crate) const WINSIZE_SIZE: usize = 8;
/// `sizeof(struct stat)`.
pub(crate) const STAT_SIZE: usize = 224;

/// `c_iflag`, `c_oflag`, `c_cflag` and `c_lflag` bits (`_termios.h`).
const IFLAG: &[(TFlag, u32)] = &[
    (TFlag::IGNBRK, 0x1),
    (TFlag::BRKINT, 0x2),
    (TFlag::IGNPAR, 0x4),
    (TFlag::PARMRK, 0x8),
    (TFlag::INPCK, 0x10),
    (TFlag::ISTRIP, 0x20),
    (TFlag::INLCR, 0x40),
    (TFlag::IGNCR, 0x80),
    (TFlag::ICRNL, 0x100),
    (TFlag::IXON, 0x200),
    (TFlag::IXOFF, 0x400),
    (TFlag::IXANY, 0x800),
    (TFlag::IMAXBEL, 0x2000),
    (TFlag::IUTF8, 0x4000),
];
const OFLAG: &[(TFlag, u32)] = &[
    (TFlag::OPOST, 0x1),
    (TFlag::ONLCR, 0x2),
    (TFlag::OXTABS, 0x4),
    (TFlag::ONOEOT, 0x8),
    (TFlag::OCRNL, 0x10),
    (TFlag::ONOCR, 0x20),
    (TFlag::ONLRET, 0x40),
];
const CFLAG: &[(TFlag, u32)] = &[
    (TFlag::CSTOPB, 0x400),
    (TFlag::CREAD, 0x800),
    (TFlag::PARENB, 0x1000),
    (TFlag::PARODD, 0x2000),
    (TFlag::HUPCL, 0x4000),
    (TFlag::CLOCAL, 0x8000),
    (TFlag::CCTS_OFLOW, 0x10000),
    (TFlag::CRTS_IFLOW, 0x20000),
];
const LFLAG: &[(TFlag, u32)] = &[
    (TFlag::ECHOKE, 0x1),
    (TFlag::ECHOE, 0x2),
    (TFlag::ECHOK, 0x4),
    (TFlag::ECHO, 0x8),
    (TFlag::ECHONL, 0x10),
    (TFlag::ECHOPRT, 0x20),
    (TFlag::ECHOCTL, 0x40),
    (TFlag::ISIG, 0x80),
    (TFlag::ICANON, 0x100),
    (TFlag::ALTWERASE, 0x200),
    (TFlag::IEXTEN, 0x400),
    (TFlag::EXTPROC, 0x800),
    (TFlag::TOSTOP, 0x400000),
    (TFlag::FLUSHO, 0x800000),
    (TFlag::NOKERNINFO, 0x2000000),
    (TFlag::PENDIN, 0x20000000),
    (TFlag::NOFLSH, 0x80000000),
];
/// `CSIZE`: bits 8 and 9 of `c_cflag`, `CS5` to `CS8`.
const CSIZE: u32 = 0x300;

/// `c_cc` indices (`V*`).
const CC: &[(Cc, usize)] = &[
    (Cc::VEOF, 0),
    (Cc::VEOL, 1),
    (Cc::VEOL2, 2),
    (Cc::VERASE, 3),
    (Cc::VWERASE, 4),
    (Cc::VKILL, 5),
    (Cc::VREPRINT, 6),
    (Cc::VERASE2, 7),
    (Cc::VINTR, 8),
    (Cc::VQUIT, 9),
    (Cc::VSUSP, 10),
    (Cc::VDSUSP, 11),
    (Cc::VSTART, 12),
    (Cc::VSTOP, 13),
    (Cc::VLNEXT, 14),
    (Cc::VDISCARD, 15),
    (Cc::VMIN, 16),
    (Cc::VTIME, 17),
    (Cc::VSTATUS, 18),
];

const _: () = assert!(NCC <= NCCS);

/// A FreeBSD `struct termios` as the kernel's settings.
pub(crate) fn termios_in(b: &[u8; TERMIOS_SIZE]) -> Termios {
    let field = |i: usize| read_u32(b, 4 * i).unwrap_or(0);
    let mut flags = TFlags::EMPTY;
    for (i, table) in [IFLAG, OFLAG, CFLAG, LFLAG].iter().enumerate() {
        let v = field(i);
        for &(f, bit) in *table {
            flags.set(f, v & bit != 0);
        }
    }
    let mut cc = [VDISABLE; NCC];
    for &(c, i) in CC {
        cc[c.index()] = b[CC_OFF + i];
    }
    let csize = (field(2) & CSIZE) >> 8;
    Termios {
        flags,
        csize: 5 + u8::try_from(csize).unwrap_or(3),
        cc,
        ispeed: read_u32(b, ISPEED_OFF).unwrap_or(9600),
        ospeed: read_u32(b, OSPEED_OFF).unwrap_or(9600),
    }
}

/// The kernel's settings as a FreeBSD `struct termios`.
pub(crate) fn termios_out(t: &Termios) -> [u8; TERMIOS_SIZE] {
    let mut b = [0u8; TERMIOS_SIZE];
    for (i, table) in [IFLAG, OFLAG, CFLAG, LFLAG].iter().enumerate() {
        let mut v = table.iter().filter(|(f, _)| t.flags.has(*f)).fold(0u32, |a, (_, bit)| a | bit);
        if i == 2 {
            v |= (u32::from(t.csize.clamp(5, 8)) - 5) << 8;
        }
        b[4 * i..4 * i + 4].copy_from_slice(&v.to_le_bytes());
    }
    // Slots FreeBSD has and the kernel does not stay disabled.
    for slot in &mut b[CC_OFF..CC_OFF + NCCS] {
        *slot = VDISABLE;
    }
    for &(c, i) in CC {
        b[CC_OFF + i] = t.cc(c);
    }
    b[ISPEED_OFF..ISPEED_OFF + 4].copy_from_slice(&t.ispeed.to_le_bytes());
    b[OSPEED_OFF..OSPEED_OFF + 4].copy_from_slice(&t.ospeed.to_le_bytes());
    b
}

/// A FreeBSD `struct winsize` (`ws_row`, `ws_col`, `ws_xpixel`,
/// `ws_ypixel`) and back.
pub(crate) fn winsize_in(b: &[u8; WINSIZE_SIZE]) -> Winsize {
    let h = |i: usize| u16::from_le_bytes([b[2 * i], b[2 * i + 1]]);
    Winsize { row: h(0), col: h(1), xpixel: h(2), ypixel: h(3) }
}

/// See [`winsize_in`].
pub(crate) fn winsize_out(w: &Winsize) -> [u8; WINSIZE_SIZE] {
    let mut b = [0u8; WINSIZE_SIZE];
    for (i, v) in [w.row, w.col, w.xpixel, w.ypixel].iter().enumerate() {
        b[2 * i..2 * i + 2].copy_from_slice(&v.to_le_bytes());
    }
    b
}

/// `S_IF*`: the type bits of `st_mode`.
const fn s_ifmt(t: FileType) -> u16 {
    match t {
        FileType::Fifo => 0o010000,
        FileType::CharDevice => 0o020000,
        FileType::Directory => 0o040000,
        FileType::BlockDevice => 0o060000,
        FileType::Regular => 0o100000,
        FileType::Symlink => 0o120000,
        FileType::Socket => 0o140000,
    }
}

/// The kernel's status as a FreeBSD `struct stat` (offsets from
/// `abidump`).
pub(crate) fn stat_out(s: &Stat) -> [u8; STAT_SIZE] {
    let mut b = [0u8; STAT_SIZE];
    let mut put = |off: usize, v: &[u8]| b[off..off + v.len()].copy_from_slice(v);
    put(0, &s.dev.to_le_bytes());
    put(8, &s.ino.to_le_bytes());
    put(16, &s.nlink.to_le_bytes());
    put(24, &(s_ifmt(s.kind) | (s.perm & 0o7777)).to_le_bytes());
    put(28, &s.uid.to_le_bytes());
    put(32, &s.gid.to_le_bytes());
    put(40, &s.rdev.to_le_bytes());
    for (off, t) in [(48, s.atime), (64, s.mtime), (80, s.ctime), (96, s.birthtime)] {
        put(off, &t.sec.to_le_bytes());
        put(off + 8, &t.nsec.to_le_bytes());
    }
    put(112, &s.size.to_le_bytes());
    put(120, &s.blocks.to_le_bytes());
    put(128, &s.blksize.to_le_bytes());
    put(132, &s.flags.to_le_bytes());
    put(136, &u64::from(s.generation).to_le_bytes());
    b
}
