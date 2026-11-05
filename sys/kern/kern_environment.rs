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

//! The kernel environment, `kenv` (FreeBSD's `kern/kern_environment.c`).
//!
//! The static environment the loader passes is parsed by
//! `sys::kenv::StaticEnv`, shared with MD code; a copy is kept for
//! `kenv(2)` ([`kenv_init`]).
//!
//! TODO(kenv): the dynamic, writable environment (`kern_setenv`).

use framework::mm::{KVec, M_WAITOK, MallocType};
use framework::sync::OnceLock;
use sys::errno::KResult;

pub use sys::kenv::StaticEnv;

/// The kernel environment (`M_KENV`).
static M_KENV: MallocType = MallocType::new("kenv");

/// The loader's environment, copied out of the boot data.
static KENV: OnceLock<KVec<u8>> = OnceLock::new();

/// Keeps a copy of the loader's environment (`init_static_kenv`).
pub fn kenv_init(envp: &[u8]) -> KResult<()> {
    let mut copy = KVec::with_capacity(envp.len(), &M_KENV, M_WAITOK)?;
    copy.extend_from_slice(envp, M_WAITOK)?;
    let _ = KENV.set(copy);
    Ok(())
}

/// The environment (empty before [`kenv_init`]).
pub fn kenv() -> StaticEnv<'static> {
    StaticEnv::new(KENV.get().map_or(&[], |v| v))
}
