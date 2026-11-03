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

//! The kernel heap: Rust's global allocator (the role of FreeBSD's
//! `malloc(9)` on top of UMA).
//!
//! * Requests up to 2 KiB are rounded up to a power of two (16 bytes to
//!   2 KiB) and served from per-size free lists. Each list is refilled by
//!   cutting a fresh page into equal objects (a minimal slab).
//! * Larger requests get their own buddy block from `vm_phys`.
//!
//! All memory is reached through the direct map. Because a request's size
//! class is a pure function of its `Layout`, `dealloc` finds the right
//! list or block order without any header.
//!
//! TODO(uma): per-CPU caches (smp), giving empty slab pages back, and
//! statistics per malloc type (`vmstat -m`).

use core::alloc::{GlobalAlloc, Layout};
use core::ptr;

use mdi::DirectMap;

use super::frame::Segment;
use super::vm_phys::VM_NFREEORDER;
use crate::sync::MtxSpin;
use crate::{Md, PAGE_SIZE};

/// Smallest object: room for the free-list link.
const KMEM_MIN: usize = 16;
/// Largest object served from a slab.
const KMEM_MAX: usize = 2048;
/// Number of size classes: 16, 32, ..., 2048.
const NCLASSES: usize = (KMEM_MAX / KMEM_MIN).trailing_zeros() as usize + 1;

/// How a layout is served.
enum Class {
    /// Slab object of size `KMEM_MIN << n`.
    Small(usize),
    /// Buddy block of `2^order` pages.
    Large(usize),
}

fn classify(layout: Layout) -> Option<Class> {
    let size = layout
        .size()
        .max(layout.align())
        .max(KMEM_MIN)
        .next_power_of_two();
    if size <= KMEM_MAX {
        return Some(Class::Small((size / KMEM_MIN).trailing_zeros() as usize));
    }
    // A buddy block of 2^order pages is aligned to its size, so an order
    // large enough for the size is large enough for the alignment.
    let order = size
        .div_ceil(PAGE_SIZE)
        .next_power_of_two()
        .trailing_zeros() as usize;
    (order < VM_NFREEORDER).then_some(Class::Large(order))
}

/// Heads of the per-class free lists (direct-map addresses, 0 = empty).
/// Each free object holds the address of the next one.
static FREELISTS: MtxSpin<[usize; NCLASSES]> = MtxSpin::new([0; NCLASSES]);

/// The kernel's global allocator.
struct Kmem;

#[global_allocator]
static KMEM: Kmem = Kmem;

// SAFETY: every block returned is either a slab object of at least the
// layout's size and alignment (objects of size `s` sit at multiples of `s`
// inside page-aligned slabs), or a buddy block, which is aligned to its own
// size; both are owned by the heap alone until freed. `dealloc` receives
// the same layout, hence the same class, so blocks return where they came
// from. All lists are protected by an interrupt-safe lock.
unsafe impl GlobalAlloc for Kmem {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        match classify(layout) {
            Some(Class::Small(c)) => {
                let mut lists = FREELISTS.lock();
                if lists[c] == 0 && !refill(&mut lists[c], KMEM_MIN << c) {
                    return ptr::null_mut();
                }
                let obj = lists[c];
                // SAFETY: `obj` is a free object on list `c`, owned by the
                // heap; its first word is the link to the next one.
                lists[c] = unsafe { (obj as *const usize).read() };
                obj as *mut u8
            }
            Some(Class::Large(order)) => match Segment::alloc(order) {
                Some(seg) => Md::phys_to_dmap(seg.leak()) as *mut u8,
                None => ptr::null_mut(),
            },
            None => ptr::null_mut(),
        }
    }

    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        match classify(layout) {
            Some(Class::Small(c)) => {
                let mut lists = FREELISTS.lock();
                // SAFETY: the caller returns an object this allocator gave
                // out for `layout`, now unused; it becomes the list head.
                unsafe { (p as *mut usize).write(lists[c]) };
                lists[c] = p as usize;
            }
            Some(Class::Large(order)) => {
                // SAFETY: `p` came from `Segment::leak` in `alloc` with this
                // order, and the caller gives it back exactly once.
                drop(unsafe { Segment::from_leaked(Md::dmap_to_phys(p as usize), order) });
            }
            None => unreachable!("kmem: dealloc of a layout alloc never served"),
        }
    }
}

/// Cuts a fresh page into objects of `size` bytes and pushes them on
/// `head`. False if no page is available.
fn refill(head: &mut usize, size: usize) -> bool {
    let Some(seg) = Segment::alloc(0) else {
        return false;
    };
    let base = Md::phys_to_dmap(seg.leak());
    for obj in (base..base + PAGE_SIZE).step_by(size).rev() {
        // SAFETY: the page was just leaked to the heap, so it owns it; each
        // object is in bounds and aligned to `size` >= 8.
        unsafe { (obj as *mut usize).write(*head) };
        *head = obj;
    }
    true
}
