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

//! `libkern`: machine-independent building blocks shared by the whole
//! kernel (the counterpart of FreeBSD's `sys/libkern`).
//!
//! This crate is part of the **trusted base**: together with `sys/arch/*`
//! it is the only place where `unsafe` is allowed. Everything here must be
//! small, portable (no `cfg(target_arch)`, no `asm!`) and exhaustively
//! justified with `// SAFETY:` comments, because every other crate builds
//! safe abstractions on top of it.
//!
//! It depends on nothing, so any layer may use it.

#![cfg_attr(not(test), no_std)]

pub mod buddy;
pub mod chacha20;
pub mod conv;
pub mod range;
pub mod rust_demangle;
pub mod spinlock;

pub use spinlock::{SpinLock, SpinLockGuard};
