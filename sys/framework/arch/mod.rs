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

//! The one place in the machine-independent kernel that names a machine:
//! which `sys/arch` crate implements the `mdi` contract for this build.
//! Every other framework file reaches it as `crate::Md`.
//!
//! Porting Nova means writing `sys/arch/<arch>`, adding its dependency to
//! `framework/Cargo.toml` and one line here.

#[cfg(target_arch = "x86_64")]
pub(crate) use amd64::Machine;

#[cfg(target_arch = "riscv64")]
pub(crate) use riscv64::Machine;

#[cfg(not(any(target_arch = "x86_64", target_arch = "riscv64")))]
compile_error!("Nova: no sys/arch port for this target architecture");
