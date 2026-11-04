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

//! FreeBSD's signal numbers, `sigset_t` and `struct sigaction`
//! (`sys/sys/signal.h`), to and from the kernel's names.

use framework::user::{FaultKind, UserFault};
use sys::bytes::{read_u32, read_u64};
use sys::signal::{NSIGRT, SigAction, SigFlags, SigHandler, SigSet, Signal};

/// FreeBSD's numbers for the named signals, in `Signal::index` order
/// (`sys/sys/signal.h`).
const NUMBERS: [i32; 33] = [
    1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27,
    28, 29, 30, 31, 32, 33,
];
/// `SIGRTMIN`.
const SIGRTMIN: i32 = 65;

/// The FreeBSD number of `sig`.
pub(crate) fn signo(sig: Signal) -> i32 {
    match sig {
        Signal::Rt(n) => SIGRTMIN + i32::from(n),
        s => NUMBERS.get(s.index()).copied().unwrap_or(0),
    }
}

/// The signal with FreeBSD number `n`, if any.
pub(crate) fn from_signo(n: i32) -> Option<Signal> {
    if let Some(i) = NUMBERS.iter().position(|&x| x == n) {
        return Signal::from_index(i);
    }
    let rt = u8::try_from(n.checked_sub(SIGRTMIN)?).ok()?;
    (rt < NSIGRT).then_some(Signal::Rt(rt))
}

/// `sizeof(sigset_t)`: 128 bits, bit `signo - 1`.
pub(crate) const SIGSET_SIZE: usize = 16;

/// A FreeBSD `sigset_t` as a kernel set (unknown numbers are dropped).
pub(crate) fn sigset_in(b: &[u8; SIGSET_SIZE]) -> SigSet {
    let bits = u128::from_le_bytes(*b);
    let mut set = SigSet::EMPTY;
    for bit in 0..128i32 {
        if bits & (1u128 << bit) != 0
            && let Some(sig) = from_signo(bit + 1)
        {
            set.insert(sig);
        }
    }
    set
}

/// A kernel set as a FreeBSD `sigset_t`.
pub(crate) fn sigset_out(set: SigSet) -> [u8; SIGSET_SIZE] {
    let mut bits = 0u128;
    for sig in set.iter() {
        if let Ok(bit) = u32::try_from(signo(sig) - 1) {
            bits |= 1u128.checked_shl(bit).unwrap_or(0);
        }
    }
    bits.to_le_bytes()
}

/// `sizeof(struct sigaction)`: handler, `sa_flags`, `sa_mask`, padding.
pub(crate) const SIGACTION_SIZE: usize = 32;
const SA_ONSTACK: i32 = 0x0001;
const SA_RESTART: i32 = 0x0002;
const SA_RESETHAND: i32 = 0x0004;
const SA_NOCLDSTOP: i32 = 0x0008;
const SA_NODEFER: i32 = 0x0010;
const SA_NOCLDWAIT: i32 = 0x0020;
const SA_SIGINFO: i32 = 0x0040;
/// `SIG_DFL` and `SIG_IGN`.
const SIG_DFL: u64 = 0;
const SIG_IGN: u64 = 1;

/// A FreeBSD `struct sigaction` as a kernel action.
pub(crate) fn sigaction_in(b: &[u8; SIGACTION_SIZE]) -> SigAction {
    let handler = read_u64(b, 0).unwrap_or(0);
    let flags = read_u32(b, 8).map_or(0, |f| i32::from_le_bytes(f.to_le_bytes()));
    let mut mask = [0u8; SIGSET_SIZE];
    if let Some(m) = b.get(12..12 + SIGSET_SIZE) {
        mask.copy_from_slice(m);
    }
    SigAction {
        handler: match handler {
            SIG_DFL => SigHandler::Default,
            SIG_IGN => SigHandler::Ignore,
            h => SigHandler::Catch(h),
        },
        mask: sigset_in(&mask),
        flags: SigFlags {
            onstack: flags & SA_ONSTACK != 0,
            restart: flags & SA_RESTART != 0,
            resethand: flags & SA_RESETHAND != 0,
            nodefer: flags & SA_NODEFER != 0,
            nocldwait: flags & SA_NOCLDWAIT != 0,
            siginfo: flags & SA_SIGINFO != 0,
            nocldstop: flags & SA_NOCLDSTOP != 0,
        },
    }
}

/// A kernel action as a FreeBSD `struct sigaction`.
pub(crate) fn sigaction_out(a: &SigAction) -> [u8; SIGACTION_SIZE] {
    let mut b = [0u8; SIGACTION_SIZE];
    let handler = match a.handler {
        SigHandler::Default => SIG_DFL,
        SigHandler::Ignore => SIG_IGN,
        SigHandler::Catch(h) => h,
    };
    let f = &a.flags;
    let flags = [
        (f.onstack, SA_ONSTACK),
        (f.restart, SA_RESTART),
        (f.resethand, SA_RESETHAND),
        (f.nocldstop, SA_NOCLDSTOP),
        (f.nodefer, SA_NODEFER),
        (f.nocldwait, SA_NOCLDWAIT),
        (f.siginfo, SA_SIGINFO),
    ]
    .iter()
    .filter(|(on, _)| *on)
    .fold(0, |acc, (_, bit)| acc | bit);
    b[0..8].copy_from_slice(&handler.to_le_bytes());
    b[8..12].copy_from_slice(&flags.to_le_bytes());
    b[12..12 + SIGSET_SIZE].copy_from_slice(&sigset_out(a.mask));
    b
}

/// The signal a user fault raises, as FreeBSD's `trap()` decides it: an
/// unmapped page is `SIGSEGV`, a protection fault (a page the program may
/// not access that way, or a general protection fault) `SIGBUS`.
pub(crate) fn trapsignal(fault: &UserFault) -> Signal {
    match fault.kind {
        FaultKind::NotMapped { .. } => Signal::SIGSEGV,
        FaultKind::Protection { .. } => Signal::SIGBUS,
        FaultKind::IllegalInstruction => Signal::SIGILL,
        FaultKind::Arithmetic => Signal::SIGFPE,
        FaultKind::Breakpoint => Signal::SIGTRAP,
        FaultKind::Other(_) => Signal::SIGBUS,
    }
}
