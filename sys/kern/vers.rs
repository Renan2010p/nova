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

//! Version strings (FreeBSD's generated `vers.c`), filled in by
//! `kern/build.rs`.

/// `kern.osrelease`: the release, `-CURRENT` while on the main branch.
pub const OSRELEASE: &str = concat!(env!("CARGO_PKG_VERSION"), "-CURRENT");
/// Source revision (`main-n12-446bf2d3e1a2`).
pub const VCS: &str = env!("NOVA_VCS");
/// Kernel configuration name (`kern.ident`), as `GENERIC` in FreeBSD.
pub const KERN_IDENT: &str = "GENERIC";
/// The compiler that built the kernel (`rustc -V`).
pub const COMPILER: &str = env!("NOVA_COMPILER");
