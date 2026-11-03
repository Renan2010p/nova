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

//! The kernel environment, `kenv` (FreeBSD's `kern/kern_environment.c`).
//!
//! The static environment the loader passes is parsed by
//! `sys::kenv::StaticEnv`, shared with MD code. The dynamic, writable
//! environment (`kenv(2)`, `kern_setenv`) will live here.

pub use sys::kenv::StaticEnv;
