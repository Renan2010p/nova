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


//! `freebsd`: FreeBSD's boot protocol (`loader(8)` to the kernel), its
//! machine-independent half: the kernel environment and how
//! `loader.conf` and `device.hints` fill it, the boot flags (`boothowto`)
//! it and the entry's `cmdline` imply. The Nova kernel speaks it, so does
//! FreeBSD's.
//!
//! The preload metadata is the kernel's own definition
//! (`sys::linker::PreloadWriter`); the machine-dependent half (loading
//! and entering the kernel) is in `arch/<arch>/<firmware>`.
//!
//! `stand.conf` options of a `protocol: freebsd` entry: `path` (the
//! kernel, `kernel_path` too), `cmdline` (`boot`'s arguments: `-s`, `-v`,
//! `name=value`, ...). TODO(freebsd): `module_path` (modules).
//!
//! 100% safe Rust.

#![no_std]
#![forbid(unsafe_code)]

pub mod conf;
pub mod env;
pub mod howto;
