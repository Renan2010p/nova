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

// The FreeBSD personality: linked in, registers itself once syscalls exist.
use compat_freebsd as _;

framework::kernel_entry!(kern::init_main::mi_startup);
