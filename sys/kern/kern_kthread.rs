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

use alloc::sync::Arc;

use framework::task::{self, Task};

/// Creates a kernel thread named `name` running `f` (`kthread_add`). The
/// thread exits when `f` returns. `None` if memory is exhausted.
pub fn kthread_add(name: &'static str, f: impl FnOnce() + Send + 'static) -> Option<Arc<Task>> {
    task::spawn(name, f)
}

/// Ends the calling kernel thread (`kthread_exit`).
pub fn kthread_exit() -> ! {
    task::exit()
}
