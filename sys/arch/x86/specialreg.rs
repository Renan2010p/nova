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

/// Local APIC base address and mode (`MSR_APICBASE`).
pub const MSR_APICBASE: u32 = 0x0000_001b;
/// `MSR_APICBASE`: the local APIC is enabled.
pub const APICBASE_ENABLED: u64 = 1 << 11;
/// `MSR_APICBASE`: the local APIC is in x2APIC mode.
pub const APICBASE_X2APIC: u64 = 1 << 10;

/// Base of the `%fs` segment.
pub const MSR_FSBASE: u32 = 0xc000_0100;
/// Base of the `%gs` segment: the current CPU's per-CPU area in the kernel.
pub const MSR_GSBASE: u32 = 0xc000_0101;
/// Value swapped into `MSR_GSBASE` by `swapgs` on kernel entry/exit.
pub const MSR_KGSBASE: u32 = 0xc000_0102;

/// `%cr4`: global pages.
pub const CR4_PGE: u64 = 1 << 7;

/// `cpuid(1).ecx`: x2APIC.
pub const CPUID2_X2APIC: u32 = 1 << 21;
/// `cpuid(1).ecx`: running under a hypervisor.
pub const CPUID2_HV: u32 = 1 << 31;
/// `cpuid(0x80000001).edx`: 1 GiB pages.
pub const AMDID_PAGE1GB: u32 = 1 << 26;
