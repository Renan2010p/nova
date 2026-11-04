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

//! Kernel threads (FreeBSD's `kern/kern_kthread.c`).

use framework::mm::KArc;
use framework::task::{self, Task};
use sys::errno::KResult;

/// Creates a kernel thread named `name` running `f` (`kthread_add`). The
/// thread exits when `f` returns. May sleep; `ENOMEM` if memory is short.
pub fn kthread_add(name: &str, f: impl FnOnce() + Send + 'static) -> KResult<KArc<Task>> {
    Ok(task::spawn(name, f)?)
}

/// Ends the calling kernel thread (`kthread_exit`).
pub fn kthread_exit() -> ! {
    task::exit()
}
