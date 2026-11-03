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

//! `x86`: machine-dependent code shared by every x86 port (FreeBSD's
//! `sys/x86`): port I/O, interrupt flag, CPUID, and later the local APIC,
//! I/O APIC, TSC and ACPI table discovery.
//!
//! This is MD code: it may use `unsafe` and `asm!`, and it must not depend
//! on any MI crate. Each architecture crate (`amd64`, one day `i386`)
//! builds its `mdi::Machine` implementation on top of it.

#![no_std]

#[cfg(not(target_arch = "x86_64"))]
compile_error!("the x86 crate only supports x86_64 targets");

pub mod cpufunc;
pub mod ns8250;
pub mod psl;
pub mod specialreg;
