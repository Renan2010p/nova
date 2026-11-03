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

//! Kernel thread stacks (FreeBSD's `vm_thread_new` / `vm_thread_dispose`).
//!
//! Each stack is `KSTACK_PAGES` pages mapped in the kernel map, below which
//! `KSTACK_GUARD_PAGES` are left unmapped: a stack overflow hits the guard
//! page and faults, instead of silently overwriting whatever lies below.
//! (The fault cannot be handled on the overflowed stack, so it becomes a
//! double fault, which runs on its own IST stack and is reported.)
//!
//! SMP: a freed stack's virtual addresses are never reused. Another CPU
//! may still hold TLB entries for them, and reusing the addresses for new
//! pages would let it write through a stale entry into a freed page.
//! Never reusing them makes stale entries harmless (nothing touches those
//! addresses again), without TLB shootdown IPIs. The kernel map has room
//! for over 26 million stacks.
//!
//! TODO(smp): recycle addresses once TLB shootdown exists.

use alloc::vec::Vec;

use mdi::KernelMap;

use super::frame::{Frame, Segment};
use crate::sync::MtxSpin;
use crate::{Md, PAGE_SIZE};

/// Pages of each kernel stack (FreeBSD amd64's `KSTACK_PAGES`).
pub const KSTACK_PAGES: usize = 4;
/// Unmapped pages below each stack (`KSTACK_GUARD_PAGES`).
pub const KSTACK_GUARD_PAGES: usize = 1;

const SLOT_SIZE: usize = (KSTACK_PAGES + KSTACK_GUARD_PAGES) * PAGE_SIZE;

/// Kernel-map slots for stacks: a bump pointer. The lock also serializes
/// `pmap_kenter`/`pmap_kremove`, as `KernelMap` requires.
struct KstackMap {
    next: usize,
}

static KSTACK_MAP: MtxSpin<KstackMap> = MtxSpin::new(KstackMap { next: Md::KVA_MIN });

/// A mapped kernel stack. Its pages are owned by it and reached only
/// through the mapping, by the thread that runs on it.
pub struct KernelStack {
    /// Start of the slot (the guard page).
    base: usize,
    frames: Vec<Frame>,
}

impl KernelStack {
    /// Allocates and maps a stack, or `None` if memory is exhausted.
    pub(crate) fn new() -> Option<Self> {
        let frames: Vec<Frame> = (0..KSTACK_PAGES)
            .map(|_| Frame::alloc())
            .collect::<Option<_>>()?;
        let mut map = KSTACK_MAP.lock();
        if map.next + SLOT_SIZE > Md::KVA_MAX {
            return None;
        }
        let base = map.next;
        map.next += SLOT_SIZE;
        let first = base + KSTACK_GUARD_PAGES * PAGE_SIZE;
        // Page-table pages are never freed: the kernel map only grows.
        let mut alloc_pt = || Segment::alloc(0).map(Segment::leak);
        for (i, frame) in frames.iter().enumerate() {
            // SAFETY: the slot is ours (taken from the map under its lock,
            // which also serializes these calls), so the page is unmapped;
            // the frame is owned by the stack being built and is used only
            // through this mapping.
            if !unsafe { Md::pmap_kenter(first + i * PAGE_SIZE, frame.paddr(), &mut alloc_pt) } {
                for j in 0..i {
                    // SAFETY: mapped just above, never used.
                    unsafe { Md::pmap_kremove(first + j * PAGE_SIZE) };
                }
                return None;
            }
        }
        Some(Self { base, frames })
    }

    /// The top of the stack (exclusive), 16-byte aligned.
    pub fn top(&self) -> usize {
        self.base + SLOT_SIZE
    }
}

impl Drop for KernelStack {
    fn drop(&mut self) {
        // Held only to serialize `pmap_kremove`, as `KernelMap` requires.
        let _map = KSTACK_MAP.lock();
        let first = self.base + KSTACK_GUARD_PAGES * PAGE_SIZE;
        for i in 0..self.frames.len() {
            // SAFETY: mapped by `new`; the stack is dropped only once its
            // thread can never run again, so nothing uses the mapping. Other
            // CPUs' stale TLB entries are harmless: the addresses are never
            // reused (see the module doc).
            unsafe { Md::pmap_kremove(first + i * PAGE_SIZE) };
        }
        // `frames` drop after this, returning the pages.
    }
}
