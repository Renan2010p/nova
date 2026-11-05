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

//! FreeBSD's conventions for the kernel core (`kern::policy`): where
//! `init` is and how single-user mode is asked for; how the loader names
//! the root and the memory disk (`vfs.root.mountfrom`, `md_image`,
//! `/dev/md0`); the console's names in `/dev` (`ttyu0`, `ttyv0`); the
//! limits a process starts with (amd64's `maxdsiz` and `maxssiz`).
//! `sys/conf` registers it.

use kern::kern_resource::{NRESOURCES, Resource, Rlimit, unlimited};
use kern::policy::SystemPolicy;

/// The FreeBSD policy.
pub struct FreebsdPolicy;

/// The instance the kernel binary registers.
pub static FREEBSD_POLICY: FreebsdPolicy = FreebsdPolicy;

impl SystemPolicy for FreebsdPolicy {
    fn init_path(&self) -> &'static str {
        "/sbin/init:/sbin/oinit:/sbin/init.bak:/rescue/init"
    }

    fn single_user_arg(&self) -> Option<&'static str> {
        Some("-s")
    }

    fn root_mountfrom_var(&self) -> Option<&'static str> {
        Some("vfs.root.mountfrom")
    }

    fn memory_disk_types(&self) -> &'static [&'static str] {
        &["md_image", "mfs_root"]
    }

    fn memory_disk_name(&self) -> &'static str {
        "/dev/md0"
    }

    fn console_aliases(&self) -> &'static [&'static [u8]] {
        &[b"ttyu0", b"ttyv0"]
    }

    fn default_limits(&self) -> [Rlimit; NRESOURCES] {
        let mut l = unlimited();
        let mut set = |r: Resource, v: u64| {
            if let Some(slot) = l.get_mut(r.index()) {
                *slot = Rlimit { cur: v, max: v };
            }
        };
        set(Resource::Data, crate::arch::MAXDSIZ);
        set(Resource::Stack, crate::arch::MAXSSIZ);
        set(Resource::MemLock, 64 << 10);
        l
    }
}
