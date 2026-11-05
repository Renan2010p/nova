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


//! `common`: the stand's core, independent of the machine, the firmware
//! and the operating system booted: the configuration file
//! ([`config`], Limine's syntax), the boot menu ([`menu`]), and reading
//! ELF files ([`load_elf`]), a format, not a system.
//!
//! 100% safe Rust. It works on byte buffers the firmware layer hands it.
//! What an entry boots, and how, is the business of its protocol
//! (`proto/<name>`); this crate never names one.

#![no_std]
#![forbid(unsafe_code)]

pub mod config;
pub mod load_elf;
pub mod menu;
