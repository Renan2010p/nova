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
//! These values are fixed by the boot protocol: the FreeBSD loader maps
//! physical memory starting at 0 to `KERNBASE` and loads the kernel at
//! physical `KERNLOAD`. `sys/conf/ldscript.amd64` must agree with them.

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
