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
//! `sys/x86`): single instructions (`cpufunc`), CPU identification, segment
//! descriptors, the i8254 timer, the local APIC, and later the I/O APIC,
//! the TSC and ACPI table discovery.
//!
//! This is MD code: it may use `unsafe` and `asm!`, and it must not depend
//! on any MI crate. Each architecture crate (`amd64`, one day `i386`)
//! builds its `mdi::Machine` implementation on top of it.

#![cfg_attr(not(test), no_std)]

#[cfg(not(target_arch = "x86_64"))]
compile_error!("the x86 crate only supports x86_64 targets");

pub mod cpufunc;
pub mod i8254;
pub mod i8259;
pub mod identcpu;
pub mod local_apic;
pub mod madt;
pub mod ns8250;
pub mod psl;
pub mod segments;
pub mod specialreg;
pub mod tsc;
