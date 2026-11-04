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

/// Extended features: `EFER_SCE` (syscall), `EFER_LME`, `EFER_NXE`.
pub const MSR_EFER: u32 = 0xc000_0080;
/// `EFER`: `syscall`/`sysret` enabled.
pub const EFER_SCE: u64 = 1 << 0;
/// Segment selectors of `syscall` (bits 47:32) and `sysret` (63:48).
pub const MSR_STAR: u32 = 0xc000_0081;
/// 64-bit `syscall` entry point.
pub const MSR_LSTAR: u32 = 0xc000_0082;
/// `%rflags` bits cleared on `syscall`.
pub const MSR_SF_MASK: u32 = 0xc000_0084;

/// Base of the `%fs` segment.
pub const MSR_FSBASE: u32 = 0xc000_0100;
/// Base of the `%gs` segment: the current CPU's per-CPU area in the kernel.
pub const MSR_GSBASE: u32 = 0xc000_0101;
/// Value swapped into `MSR_GSBASE` by `swapgs` on kernel entry/exit.
pub const MSR_KGSBASE: u32 = 0xc000_0102;

/// `%cr0`: monitor coprocessor (`wait` honours `TS`).
pub const CR0_MP: u64 = 1 << 1;
/// `%cr0`: emulate the FPU (traps every FPU instruction).
pub const CR0_EM: u64 = 1 << 2;
/// `%cr0`: task switched (the next FPU instruction traps).
pub const CR0_TS: u64 = 1 << 3;
/// `%cr0`: native FPU error reporting.
pub const CR0_NE: u64 = 1 << 5;

/// `%cr4`: global pages.
pub const CR4_PGE: u64 = 1 << 7;
/// `%cr4`: the OS saves SSE state with `fxsave`/`fxrstor`.
pub const CR4_OSFXSR: u64 = 1 << 9;
/// `%cr4`: the OS handles SIMD floating-point exceptions (`#XM`).
pub const CR4_OSXMMEXCPT: u64 = 1 << 10;

/// `cpuid(1).ecx`: x2APIC.
pub const CPUID2_X2APIC: u32 = 1 << 21;
/// `cpuid(1).ecx`: running under a hypervisor.
pub const CPUID2_HV: u32 = 1 << 31;
/// `cpuid(0x80000001).edx`: 1 GiB pages.
pub const AMDID_PAGE1GB: u32 = 1 << 26;
