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

//! The kernel heap: the back end of [`super::malloc`] (the role of UMA
//! under FreeBSD's `malloc(9)`). It is not Rust's global allocator: the
//! kernel has none.
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
//! TODO(uma): per-CPU caches (smp), giving empty slab pages back.

use libkern::conv::usize_from_u32;
use core::alloc::Layout;
use core::ptr::NonNull;

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
const NCLASSES: usize = usize_from_u32((KMEM_MAX / KMEM_MIN).trailing_zeros()) + 1;

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
        return Some(Class::Small(usize_from_u32((size / KMEM_MIN).trailing_zeros())));
    }
    // A buddy block of 2^order pages is aligned to its size, so an order
    // large enough for the size is large enough for the alignment.
    let order = usize_from_u32(size
        .div_ceil(PAGE_SIZE)
        .next_power_of_two()
        .trailing_zeros());
    (order < VM_NFREEORDER).then_some(Class::Large(order))
}

/// Heads of the per-class free lists (direct-map addresses, 0 = empty).
/// Each free object holds the address of the next one.
static FREELISTS: MtxSpin<[usize; NCLASSES]> = MtxSpin::new([0; NCLASSES]);

/// A block of at least `layout.size()` bytes aligned to `layout.align()`
/// (a non-zero size), owned by the caller until [`kmem_free`]; `None` if
/// memory is exhausted or the layout too large.
///
/// Every block is either a slab object, which sits at a multiple of its
/// size inside a page-aligned slab, or a buddy block, aligned to its own
/// size; so both satisfy the alignment.
pub(crate) fn kmem_alloc(layout: Layout) -> Option<NonNull<u8>> {
    match classify(layout)? {
        Class::Small(c) => {
            let mut lists = FREELISTS.lock();
            let head = lists.get_mut(c)?;
            if *head == 0 && !refill(head, KMEM_MIN << c) {
                return None;
            }
            let obj = *head;
            // SAFETY: `obj` is a free object on this list, owned by the heap;
            // its first word is the link to the next one.
            *head = unsafe { core::ptr::with_exposed_provenance::<usize>(obj).read() };
            NonNull::new(core::ptr::with_exposed_provenance_mut::<u8>(obj))
        }
        Class::Large(order) => {
            let seg = Segment::alloc(order)?;
            NonNull::new(core::ptr::with_exposed_provenance_mut::<u8>(Md::phys_to_dmap(seg.leak())))
        }
    }
}

/// Gives back a block from [`kmem_alloc`].
///
/// # Safety
///
/// `p` must come from `kmem_alloc(layout)` with this same layout, be
/// unused, and be freed only once.
pub(crate) unsafe fn kmem_free(p: NonNull<u8>, layout: Layout) {
    match classify(layout) {
        Some(Class::Small(c)) => {
            let mut lists = FREELISTS.lock();
            let Some(head) = lists.get_mut(c) else {
                unreachable!("kmem_free: no size class {c}");
            };
            // SAFETY: the caller returns an object this heap gave out for
            // `layout`, now unused; it becomes the list head.
            unsafe { p.cast::<usize>().write(*head) };
            *head = p.as_ptr().expose_provenance();
        }
        Some(Class::Large(order)) => {
            // SAFETY: `p` came from `Segment::leak` in `kmem_alloc` with this
            // order, and the caller gives it back exactly once.
            drop(unsafe { Segment::from_leaked(Md::dmap_to_phys(p.as_ptr().expose_provenance()), order) });
        }
        None => unreachable!("kmem_free: a layout kmem_alloc never served"),
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
        unsafe { core::ptr::with_exposed_provenance_mut::<usize>(obj).write(*head) };
        *head = obj;
    }
    true
}
