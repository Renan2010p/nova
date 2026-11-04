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

/// `%rflags` while running in the kernel: interrupts off, DF clear.
pub const PSL_KERNEL: u64 = PSL_RESERVED_DEFAULT;
