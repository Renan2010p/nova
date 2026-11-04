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

//! The physical page allocator (FreeBSD's `vm/vm_phys.c`).
//!
//! The buddy algorithm is `libkern::buddy`; this module gives it its
//! storage. The free-list links live inside the free pages, reached
//! through the direct map, and the one-byte-per-page state array is carved
//! out of the first large enough `physavail` range at boot.
//!
//! Everything below is private to the framework: services get pages only
//! as [`Frame`](super::Frame)s and [`Segment`](super::Segment)s.

use libkern::buddy::{Buddy, Links};
use mdi::{DirectMap, PhysRange};

use crate::sync::MtxSpin;
use crate::{Md, PAGE_SIZE};

/// Number of block orders, from the machine (`VM_NFREEORDER`).
pub const VM_NFREEORDER: usize = crate::VM_NFREEORDER;

const PAGE_SHIFT: u32 = PAGE_SIZE.trailing_zeros();

/// Free-list links stored in the first 16 bytes of each free block.
struct DmapLinks;

impl Links for DmapLinks {
    fn get(&mut self, pfn: u64) -> (u64, u64) {
        let p = Md::phys_to_dmap(pfn << PAGE_SHIFT) as *const u64;
        // SAFETY: `pfn` heads a free block: the allocator owns that page and
        // nothing else reads or writes it. The direct map makes it readable
        // (mdi::DirectMap contract), and the address is page aligned.
        unsafe { (p.read(), p.add(1).read()) }
    }

    fn set(&mut self, pfn: u64, next: u64, prev: u64) {
        let p = Md::phys_to_dmap(pfn << PAGE_SHIFT) as *mut u64;
        // SAFETY: as in `get`; the page is being put on (or is on) a free
        // list, so the allocator is its only user.
        unsafe {
            p.write(next);
            p.add(1).write(prev);
        }
    }
}

static VM_PHYS: MtxSpin<Option<Buddy<'static, DmapLinks, VM_NFREEORDER>>> = MtxSpin::new(None);

/// Builds the allocator from `physavail`. Called once by `framework::__init`.
pub(crate) fn init(physavail: &[PhysRange]) {
    let mut vm = VM_PHYS.lock();
    assert!(vm.is_none(), "vm_phys: initialized twice");

    let first_pfn = |r: &PhysRange| r.start.div_ceil(PAGE_SIZE as u64);
    let end_pfn = |r: &PhysRange| r.end.min(Md::dmap_limit()) / PAGE_SIZE as u64;
    let npages = physavail.iter().map(end_pfn).max().unwrap_or(0);
    let state_pages = (npages as usize).div_ceil(PAGE_SIZE) as u64;

    // Steal room for the state array from the start of the first range
    // that has it (FreeBSD's vm_page_startup does the same for vm_page_array).
    let (host, state_pfn) = physavail
        .iter()
        .enumerate()
        .find(|(_, r)| end_pfn(r).saturating_sub(first_pfn(r)) > state_pages)
        .map(|(i, r)| (i, first_pfn(r)))
        .expect("vm_phys: no physical memory for the page state array");
    let state_va = Md::phys_to_dmap(state_pfn << PAGE_SHIFT) as *mut u8;
    // SAFETY: these pages are available RAM (physavail) inside the direct
    // map, taken out of the allocator below, so this slice is their only
    // user for the life of the kernel. Zeroed before use.
    let state = unsafe {
        core::ptr::write_bytes(state_va, 0, npages as usize);
        core::slice::from_raw_parts_mut(state_va, npages as usize)
    };

    let mut buddy = Buddy::new(state, DmapLinks);
    for (i, r) in physavail.iter().enumerate() {
        let start = if i == host {
            state_pfn + state_pages
        } else {
            first_pfn(r)
        };
        buddy.add_range(start, end_pfn(r));
    }
    *vm = Some(buddy);
}

/// Allocates `2^order` physically contiguous pages, aligned to their size;
/// returns the first page frame number. `None` if out of memory, or before
/// the allocator is initialized.
pub(crate) fn alloc(order: usize) -> Option<u64> {
    VM_PHYS.lock().as_mut()?.alloc(order)
}

/// Frees a block returned by [`alloc`] with the same order.
pub(crate) fn free(pfn: u64, order: usize) {
    VM_PHYS
        .lock()
        .as_mut()
        .expect("vm_phys: free before init")
        .free(pfn, order);
}

/// Number of free pages (FreeBSD's `vm_free_count()`).
pub fn free_count() -> u64 {
    VM_PHYS.lock().as_ref().map_or(0, |b| b.free_pages())
}
