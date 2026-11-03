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

//! Stack traces (FreeBSD's `amd64/db_trace.c`, `stack_save`).
//!
//! The kernel is built with frame pointers, so every frame starts with
//! the caller's `%rbp` followed by the return address. The walk checks
//! that each word is mapped kernel memory before reading it, so a corrupt
//! chain ends the trace instead of faulting inside a panic.

use crate::pmap::pmap_kextract;

/// Deepest trace printed.
const MAX_FRAMES: usize = 32;

/// Lowest kernel address on amd64.
const VM_MIN_KERNEL: usize = 0xffff_8000_0000_0000;

fn readable(va: usize) -> bool {
    va >= VM_MIN_KERNEL && va.is_multiple_of(8) && pmap_kextract(va).is_some()
}

/// `mdi::Cpu::stack_trace`.
#[inline(never)]
pub(crate) fn stack_trace(f: &mut dyn FnMut(usize) -> bool) {
    let mut rbp: usize;
    // SAFETY: reads a register.
    unsafe {
        core::arch::asm!("mov {}, rbp", out(reg) rbp, options(nomem, nostack, preserves_flags))
    };
    for _ in 0..MAX_FRAMES {
        if rbp == 0 || !readable(rbp) || !readable(rbp + 8) {
            break;
        }
        // SAFETY: both words are mapped kernel memory (checked above) and
        // are only read, as plain integers.
        let (next, ret) = unsafe {
            (
                (rbp as *const usize).read(),
                ((rbp + 8) as *const usize).read(),
            )
        };
        if ret == 0 || !f(ret) {
            break;
        }
        // Callers' frames are at higher addresses; anything else is a loop
        // or garbage.
        if next <= rbp {
            break;
        }
        rbp = next;
    }
}
