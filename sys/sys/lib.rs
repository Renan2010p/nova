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

//! `sys`: machine-independent definitions shared by every layer of the
//! kernel, the counterpart of FreeBSD's `sys/sys/*.h` headers.
//!
//! Like those headers, this crate is included by MD *and* MI code, so it
//! holds only types, constants and small pure functions over byte slices.
//! It never touches hardware and never contains `unsafe`.
//!
//! What belongs here: things whose meaning does not depend on the CPU,
//! such as the boot protocol records ([`linker`]) or the `boothowto` flags
//! ([`reboot`]). What does not: anything that changes per architecture
//! (that is `sys/arch/*`) or per userland ABI (that is `sys/compat/*`).

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

pub mod bytes;
pub mod copyright;
pub mod linker;
pub mod physmem;
pub mod reboot;
