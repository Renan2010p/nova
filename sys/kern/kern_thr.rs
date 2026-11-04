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

//! Threads as programs see them (FreeBSD's `kern/kern_thr.c`).
//!
//! TODO(thr): `thr_new`, `thr_exit`, `thr_kill`: processes have one thread.

/// The calling thread's id (`thr_self`).
pub fn kern_thr_self() -> u64 {
    framework::task::current().tid()
}
