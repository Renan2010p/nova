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

//! Where interrupts enter the framework (the MI half of FreeBSD's
//! `intr_event` dispatch).
//!
//! The MD trap code acknowledges the interrupt controller and calls
//! [`__nova_intr`] by name. The symbol is the upcall declared in `mdi`
//! (`IntrUpcall`), and its type is checked against that declaration below,
//! so the MD side's `extern` declaration can rely on it.

use mdi::Interrupt;

use crate::cpu::intr_guard;

/// The interrupt upcall (`mdi::IntrUpcall`). Not for services.
#[doc(hidden)]
#[unsafe(no_mangle)]
pub fn __nova_intr(intr: Interrupt) {
    // Interrupts are already off; the guard records it, so code called
    // from here holds a `PinnedToCpu` proof and cannot re-enable them.
    let _intr = intr_guard();
    match intr {
        Interrupt::Clock => crate::clock::clock_intr(),
        _ => panic!("__nova_intr: unhandled {intr:?}"),
    }
}

const _: mdi::IntrUpcall = __nova_intr;

/// The interrupt exit upcall (`mdi::IntrExitUpcall`): runs on the
/// interrupted thread's stack, interrupts off, no guard held; switches
/// threads if a preemption was requested. Not for services.
#[doc(hidden)]
#[unsafe(no_mangle)]
pub fn __nova_intr_exit() {
    crate::task::preempt_point();
}

const _: mdi::IntrExitUpcall = __nova_intr_exit;
