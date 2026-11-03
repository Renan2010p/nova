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

//! Per-CPU variables (FreeBSD's `DPCPU`), safe for services to use.
//!
//! Each CPU has its own slot. Reaching *your* slot requires a
//! [`PinnedToCpu`] guard, so the compiler enforces that you cannot be
//! moved to another CPU (or interrupted by code using the same slot)
//! while holding the reference. Reading *another* CPU's slot is allowed
//! only when the data is `Sync`.
//!
//! Today every variable reserves `MAXCPU` slots statically. When the VM
//! system exists, slots will be allocated per CPU actually present, as
//! FreeBSD's DPCPU does; this API will not change.

use crate::MAXCPU;
use crate::cpu::{CpuId, PinnedToCpu, curcpu};

/// One `T` per CPU.
///
/// ```ignore
/// static TICKS: CpuLocal<Cell<u64>> = CpuLocal::new([const { Cell::new(0) }; MAXCPU]);
/// let g = framework::cpu::intr_guard();
/// let t = TICKS.get(&g);
/// t.set(t.get() + 1);
/// ```
pub struct CpuLocal<T> {
    slots: [T; MAXCPU],
}

// SAFETY: a slot is reached either by its own CPU through `get`, which
// requires a `PinnedToCpu` guard (interrupts off: no other code can run on
// this CPU, and no migration while the borrow lives), or by any CPU
// through `get_on`, which requires `T: Sync`. So a `!Sync` `T` is never
// accessed from two contexts at once; it only needs to be `Send`, since
// slots are created on the boot CPU and used on the others.
unsafe impl<T: Send> Sync for CpuLocal<T> {}

impl<T> CpuLocal<T> {
    /// Creates the variable from its initial per-CPU values.
    pub const fn new(slots: [T; MAXCPU]) -> Self {
        Self { slots }
    }

    /// This CPU's slot, valid while the guard is held.
    pub fn get<'a>(&'a self, _pinned: &'a impl PinnedToCpu) -> &'a T {
        &self.slots[curcpu().index()]
    }
}

impl<T: Sync> CpuLocal<T> {
    /// Any CPU's slot. Only for `Sync` data (atomics, locks, ...).
    pub fn get_on(&self, cpu: CpuId) -> &T {
        &self.slots[cpu.index()]
    }
}
