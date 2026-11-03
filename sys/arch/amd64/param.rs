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

//! Machine parameters (FreeBSD's `amd64/include/param.h`).

/// `hw.machine`.
pub const MACHINE: &str = "amd64";
/// `hw.machine_arch`.
pub const MACHINE_ARCH: &str = "amd64";

/// Maximum number of CPUs (FreeBSD amd64 allows 1024; 256 is plenty for
/// now and keeps statically sized per-CPU tables small).
pub const MAXCPU: usize = 256;

/// log2 of the base page size.
pub const PAGE_SHIFT: usize = 12;
/// Base page size.
pub const PAGE_SIZE: usize = 1 << PAGE_SHIFT;
