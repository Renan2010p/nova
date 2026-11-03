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

//! Kernel virtual memory layout (FreeBSD's `amd64/include/vmparam.h`).
//!
//! `KERNBASE` and `KERNLOAD` are fixed by the boot protocol: the FreeBSD
//! loader maps physical memory starting at 0 to `KERNBASE` and loads the
//! kernel at physical `KERNLOAD`. `sys/conf/ldscript.amd64` must agree with
//! them. The direct map is placed where FreeBSD places it.

/// Virtual address that maps physical address 0 at entry (the top 2 GiB).
pub const KERNBASE: u64 = 0xffff_ffff_8000_0000;

/// Physical load address of the kernel image.
pub const KERNLOAD: u64 = 0x20_0000;

/// Virtual address of the first byte of the kernel image.
pub const KERNSTART: u64 = KERNBASE + KERNLOAD;

/// How much of physical memory the loader's page tables map at
/// `KERNBASE`. Anything we touch before building our own page tables
/// (kernel, modules, metadata) must lie below this.
pub const BOOT_MAPPED: u64 = 1 << 30;

/// PML4 slot of the kernel (`KPML4I`): the top 512 GiB.
pub const KPML4I: usize = 511;
/// PDP slot of `KERNBASE` within `KPML4I` (`KPDPI`): the top 2 GiB.
pub const KPDPI: usize = 510;

/// First PML4 slot of the direct map (`DMPML4I`).
pub const DMPML4I: usize = 496;
/// Number of PML4 slots reserved for the direct map (`NDMPML4E`): 4 TiB.
pub const NDMPML4E: usize = 8;
/// Start of the direct map (`DMAP_MIN_ADDRESS`).
pub const DMAP_MIN_ADDRESS: u64 = 0xffff_0000_0000_0000 | ((DMPML4I as u64) << 39);

const _: () = assert!(DMAP_MIN_ADDRESS == 0xffff_f800_0000_0000);

/// Start of the kernel map (`VM_MIN_KERNEL_ADDRESS`: PML4 slot 508,
/// FreeBSD's `KPML4BASE`), where the framework maps kernel stacks.
pub const VM_MIN_KERNEL_ADDRESS: usize = 0xffff_fe00_0000_0000;
/// End of the part of the kernel map the framework manages (one PML4 slot).
pub const VM_MAX_KERNEL_MAP: usize = VM_MIN_KERNEL_ADDRESS + (1 << 39);
