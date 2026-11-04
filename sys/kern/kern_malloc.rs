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

//! The kernel's general malloc types (FreeBSD's `kern/kern_malloc.c`).
//! Subsystems define their own next to their code; these are for short
//! lived or miscellaneous memory. Allocation itself is the framework's
//! (`framework::mm::malloc`).

use framework::mm::MallocType;

/// Short-lived memory, freed before the operation that took it returns
/// (`M_TEMP`).
pub static M_TEMP: MallocType = MallocType::new("temp");
