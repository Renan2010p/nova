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

//! The FreeBSD conventions for the first user program: the default
//! `init_path` and the `-s` of single-user mode. `sys/conf` registers
//! this policy, so `kern::init_main` itself does not spell FreeBSD's
//! userland out.

use kern::init_main::InitPolicy;

/// The FreeBSD init policy.
pub struct FreebsdInit;

/// The instance the kernel binary registers.
pub static FREEBSD_INIT: FreebsdInit = FreebsdInit;

impl InitPolicy for FreebsdInit {
    fn init_path(&self) -> &'static str {
        "/sbin/init:/sbin/oinit:/sbin/init.bak:/rescue/init"
    }

    fn single_user_arg(&self) -> Option<&'static str> {
        Some("-s")
    }
}
