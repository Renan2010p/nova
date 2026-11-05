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

//! The Nova kernel binary.
//!
//! This crate only *assembles* the kernel, like a FreeBSD kernel config
//! (`GENERIC`) does: it picks the subsystems and personalities to link in
//! and binds the MI entry point. It contains no logic. Optional parts will
//! become Cargo features here (the equivalent of `options` and `device`
//! lines).

#![no_std]
#![no_main]
#![deny(unsafe_code)]

/// Registers the ABIs, executable formats, file systems and the system
/// policy this kernel is built with, then starts the kernel. This is the
/// only place that names them (rules R9, R15, R19 and R20).
fn mi_startup(boot: &framework::BootInfo<'_>) -> ! {
    kern::kern_abi::abi_register(&compat_freebsd::ELF64_FREEBSD_SYSVEC);
    kern::kern_exec::register(&exec_elf::ELF64_ACTIVATOR);
    kern::vfs_mount::vfs_register(&ufs::UFS_VFSCONF);
    kern::vfs_mount::vfs_register(&tmpfs::TMPFS_VFSCONF);
    kern::policy::set_system_policy(&compat_freebsd::FREEBSD_POLICY);
    kern::init_main::mi_startup(boot)
}

framework::kernel_entry!(mi_startup);
