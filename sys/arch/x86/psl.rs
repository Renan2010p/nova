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

/// Carry flag. The FreeBSD syscall ABI reports errors with it set.
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

/// `%rflags` while running in the kernel: interrupts off, DF clear.
pub const PSL_KERNEL: u64 = PSL_RESERVED_DEFAULT;
