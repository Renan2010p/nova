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

//! Processor status long-word: `%rflags` bits (FreeBSD's `x86/psl.h`).

/// Carry flag.
pub const PSL_C: u64 = 0x0000_0001;
/// Reserved bit, always 1.
pub const PSL_RESERVED_DEFAULT: u64 = 0x0000_0002;
/// Trace (single-step) flag.
pub const PSL_T: u64 = 0x0000_0100;
/// Interrupt enable flag.
pub const PSL_I: u64 = 0x0000_0200;
/// Direction flag.
pub const PSL_D: u64 = 0x0000_0400;
/// Resume flag.
pub const PSL_RF: u64 = 0x0001_0000;

/// Parity flag.
pub const PSL_PF: u64 = 0x0000_0004;
/// Auxiliary carry flag.
pub const PSL_AF: u64 = 0x0000_0010;
/// Zero flag.
pub const PSL_Z: u64 = 0x0000_0040;
/// Sign flag.
pub const PSL_N: u64 = 0x0000_0080;
/// Overflow flag.
pub const PSL_V: u64 = 0x0000_0800;
/// I/O privilege level.
pub const PSL_IOPL: u64 = 0x0000_3000;
/// Nested task.
pub const PSL_NT: u64 = 0x0000_4000;
/// Alignment check.
pub const PSL_AC: u64 = 0x0004_0000;
/// CPUID available.
pub const PSL_ID: u64 = 0x0020_0000;

/// The flags user mode may change (FreeBSD's `PSL_USERCHANGE`).
pub const PSL_USERCHANGE: u64 =
    PSL_C | PSL_PF | PSL_AF | PSL_Z | PSL_N | PSL_T | PSL_D | PSL_V | PSL_AC | PSL_ID;
/// `%rflags` of a new user program (`PSL_USER`): interrupts on.
pub const PSL_USER: u64 = PSL_RESERVED_DEFAULT | PSL_I;

/// The `%rflags` user mode runs with, from what the program asked for
/// (`regs->tf_rflags` checked by FreeBSD's `set_regs`/`sigreturn`): only
/// [`PSL_USERCHANGE`] bits are taken from it; interrupts are on, IOPL is 0,
/// and no nested-task or virtual-8086 mode.
pub const fn user_rflags(requested: u64) -> u64 {
    (requested & PSL_USERCHANGE) | PSL_USER
}

/// `%rflags` while running in the kernel: interrupts off, DF clear.
pub const PSL_KERNEL: u64 = PSL_RESERVED_DEFAULT;

/// Proofs (`make kani`).
#[cfg(kani)]
mod verification {
    use super::*;

    /// Virtual-8086 mode.
    const PSL_VM: u64 = 0x0002_0000;

    /// Whatever a program puts in its flags, user mode runs with
    /// interrupts on, IOPL 0 (no port access, no `cli`), outside nested-task
    /// and virtual-8086 mode, with the reserved bit set and no bit above
    /// those the CPU defines.
    #[kani::proof]
    fn user_rflags_is_safe() {
        let r = user_rflags(kani::any());
        assert!(r & PSL_I != 0);
        assert!(r & PSL_IOPL == 0);
        assert!(r & PSL_NT == 0);
        assert!(r & PSL_VM == 0);
        assert!(r & PSL_RF == 0);
        assert!(r & PSL_RESERVED_DEFAULT != 0);
        assert!(r >> 22 == 0);
    }

    /// The flags a program may set are kept.
    #[kani::proof]
    fn user_rflags_keeps_user_bits() {
        let req: u64 = kani::any();
        assert_eq!(user_rflags(req) & PSL_USERCHANGE, req & PSL_USERCHANGE);
    }
}
