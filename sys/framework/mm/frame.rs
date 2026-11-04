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

//! Owned physical pages.
//!
//! A [`Segment`] is `2^order` physically contiguous pages; a [`Frame`] is
//! a segment of one page. Each is the unique owner of its memory: it can
//! only be obtained from the allocator, cannot be cloned, and frees the
//! pages when dropped. Contents are always zeroed on allocation, so no
//! handle ever exposes another owner's old data.

use mdi::DirectMap;

use super::vm_phys;
use crate::{Md, PAGE_SIZE};

/// `2^order` physically contiguous, size-aligned pages.
#[derive(Debug)]
pub struct Segment {
    pfn: u64,
    order: usize,
}

impl Segment {
    /// Allocates and zeroes `2^order` contiguous pages, or `None` if there
    /// is no such block free.
    pub fn alloc(order: usize) -> Option<Self> {
        if order >= vm_phys::VM_NFREEORDER {
            return None;
        }
        let seg = Self {
            pfn: vm_phys::alloc(order)?,
            order,
        };
        // SAFETY: the allocator just handed these pages to `seg`, which owns
        // them exclusively; they are inside the direct map.
        unsafe { core::ptr::write_bytes(seg.va(), 0, seg.size()) };
        Some(seg)
    }

    /// Physical address of the first byte.
    pub fn paddr(&self) -> u64 {
        self.pfn * PAGE_SIZE as u64
    }

    /// Size in bytes.
    pub fn size(&self) -> usize {
        PAGE_SIZE << self.order
    }

    /// Block order: the segment is `2^order` pages.
    pub fn order(&self) -> usize {
        self.order
    }

    fn va(&self) -> *mut u8 {
        Md::phys_to_dmap(self.paddr()) as *mut u8
    }

    /// Copies `buf.len()` bytes at `offset` into `buf`.
    ///
    /// # Panics
    ///
    /// If the range is outside the segment.
    pub fn read_bytes(&self, offset: usize, buf: &mut [u8]) {
        assert!(
            offset
                .checked_add(buf.len())
                .is_some_and(|end| end <= self.size())
        );
        // SAFETY: in bounds (checked); this segment owns the memory, and
        // `&self` excludes a concurrent `write_bytes`.
        unsafe {
            core::ptr::copy_nonoverlapping(self.va().add(offset), buf.as_mut_ptr(), buf.len())
        }
    }

    /// Copies `buf` into the segment at `offset`.
    ///
    /// # Panics
    ///
    /// If the range is outside the segment.
    pub fn write_bytes(&mut self, offset: usize, buf: &[u8]) {
        assert!(
            offset
                .checked_add(buf.len())
                .is_some_and(|end| end <= self.size())
        );
        // SAFETY: in bounds (checked); `&mut self` makes this access exclusive.
        unsafe { core::ptr::copy_nonoverlapping(buf.as_ptr(), self.va().add(offset), buf.len()) }
    }

    /// Gives the pages up forever, returning their physical address. For
    /// the framework's own long-lived structures (e.g. heap slabs).
    pub(crate) fn leak(self) -> u64 {
        let pa = self.paddr();
        core::mem::forget(self);
        pa
    }

    /// Re-takes ownership of a block given up by [`Segment::leak`].
    ///
    /// # Safety
    ///
    /// `pa` and `order` must come from a leaked segment that no one uses
    /// anymore, and each leaked segment may be re-taken only once.
    pub(crate) unsafe fn from_leaked(pa: u64, order: usize) -> Self {
        Self {
            pfn: pa / PAGE_SIZE as u64,
            order,
        }
    }
}

impl Drop for Segment {
    fn drop(&mut self) {
        vm_phys::free(self.pfn, self.order);
    }
}

/// One owned physical page.
#[derive(Debug)]
pub struct Frame(Segment);

impl Frame {
    /// Allocates one zeroed page, or `None` if memory is exhausted.
    pub fn alloc() -> Option<Self> {
        Segment::alloc(0).map(Frame)
    }

    /// Physical address of the page.
    pub fn paddr(&self) -> u64 {
        self.0.paddr()
    }

    /// Copies `buf.len()` bytes at `offset` into `buf`. Panics if out of
    /// the page.
    pub fn read_bytes(&self, offset: usize, buf: &mut [u8]) {
        self.0.read_bytes(offset, buf)
    }

    /// Copies `buf` into the page at `offset`. Panics if out of the page.
    pub fn write_bytes(&mut self, offset: usize, buf: &[u8]) {
        self.0.write_bytes(offset, buf)
    }

    /// Gives the page up, returning its physical address (see
    /// [`Segment::leak`]). For page-table pages handed to the MD layer.
    pub(crate) fn leak(self) -> u64 {
        self.0.leak()
    }

    /// Re-takes ownership of a page given up by [`Frame::leak`].
    ///
    /// # Safety
    ///
    /// As for [`Segment::from_leaked`].
    pub(crate) unsafe fn from_leaked(pa: u64) -> Self {
        // SAFETY: forwarded to the caller.
        Frame(unsafe { Segment::from_leaked(pa, 0) })
    }
}
