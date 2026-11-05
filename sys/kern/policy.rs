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


//! The conventions of the system Nova runs, which the kernel core does
//! not hardcode: a personality supplies them ([`SystemPolicy`]), and
//! `sys/conf` registers it before `mi_startup` ([`set_system_policy`]).
//!
//! The kernel knows mechanisms (start a process, mount a root, name a
//! device); a system decides their names and defaults: where `init` is,
//! what the loader calls a memory disk, how the serial console is named
//! in `/dev`, what limits a process starts with. Another system is
//! another policy, the core unchanged. (The memory layout of a program is
//! its ABI's: `kern::kern_abi::ExecAbi`.)

use framework::sync::OnceLock;

use crate::kern_resource::{NRESOURCES, Rlimit, unlimited};

/// How the system starts and names things.
pub trait SystemPolicy: Sync {
    /// The paths tried for `init` when the environment has no
    /// `init_path`, `:`-separated (FreeBSD's is `/sbin/init:...`).
    fn init_path(&self) -> &'static str;

    /// The argument single-user mode adds after the program name (`-s`
    /// for FreeBSD); `None` if the system has no such mode.
    fn single_user_arg(&self) -> Option<&'static str>;

    /// The environment variable naming the root file system, as
    /// `<type>:<device>[;...]` (FreeBSD's `vfs.root.mountfrom`); `None`
    /// to try every type on the memory disk.
    fn root_mountfrom_var(&self) -> Option<&'static str>;

    /// The types the loader gives a preloaded memory disk image
    /// (FreeBSD's `md_image`, `mfs_root`).
    fn memory_disk_types(&self) -> &'static [&'static str];

    /// The device name of the memory disk (FreeBSD's `/dev/md0`).
    fn memory_disk_name(&self) -> &'static str;

    /// More names the console has in `/dev`, besides `console` (FreeBSD's
    /// serial line `ttyu0`, first virtual terminal `ttyv0`).
    fn console_aliases(&self) -> &'static [&'static [u8]];

    /// The resource limits the first process starts with (the others
    /// inherit theirs).
    fn default_limits(&self) -> [Rlimit; NRESOURCES] {
        unlimited()
    }
}

/// A policy with no conventions at all: `init` at `/sbin/init`, nothing
/// else named. Used until one is registered (and by tests).
struct NoPolicy;

impl SystemPolicy for NoPolicy {
    fn init_path(&self) -> &'static str {
        "/sbin/init"
    }

    fn single_user_arg(&self) -> Option<&'static str> {
        None
    }

    fn root_mountfrom_var(&self) -> Option<&'static str> {
        None
    }

    fn memory_disk_types(&self) -> &'static [&'static str] {
        &[]
    }

    fn memory_disk_name(&self) -> &'static str {
        "md0"
    }

    fn console_aliases(&self) -> &'static [&'static [u8]] {
        &[]
    }
}

static NO_POLICY: NoPolicy = NoPolicy;
static POLICY: OnceLock<&'static dyn SystemPolicy> = OnceLock::new();

/// Registers the system's policy. Called by the kernel binary before
/// `mi_startup`.
///
/// # Panics
///
/// If called more than once: the kernel binary is misconfigured.
pub fn set_system_policy(policy: &'static dyn SystemPolicy) {
    assert!(POLICY.set(policy).is_ok(), "system policy set twice");
}

/// The registered policy (one with no conventions if none is).
pub fn policy() -> &'static dyn SystemPolicy {
    POLICY.get().copied().unwrap_or(&NO_POLICY)
}
