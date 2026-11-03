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

//! Model-specific registers and control register bits (FreeBSD's
//! `x86/include/specialreg.h`).

/// Base of the `%fs` segment.
pub const MSR_FSBASE: u32 = 0xc000_0100;
/// Base of the `%gs` segment: the current CPU's per-CPU area in the kernel.
pub const MSR_GSBASE: u32 = 0xc000_0101;
/// Value swapped into `MSR_GSBASE` by `swapgs` on kernel entry/exit.
pub const MSR_KGSBASE: u32 = 0xc000_0102;
