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

//! A binary buddy allocator over page frame numbers, the algorithm of
//! FreeBSD's `vm/vm_phys.c` free lists.
//!
//! Blocks of `2^order` pages are kept on one free list per order. A block
//! is split in halves to satisfy smaller requests, and merged with its
//! *buddy* (the other half of its parent) when both are free.
//!
//! This module is only the algorithm, and it is entirely safe code: where
//! the list links live is abstracted by [`Links`]. The framework stores
//! them inside the free pages themselves, through the direct map; the unit
//! tests store them in a vector. One byte of state per page records
//! whether it heads a free block, and of which order.

/// "No page": the end of a free list.
pub const NIL: u64 = u64::MAX;

/// State byte of a page that heads a free block; the low bits hold the
/// block's order.
const FREE: u8 = 0x80;

/// Storage for the `(next, prev)` links of free blocks, indexed by the
/// block's first page frame number.
pub trait Links {
    /// Reads the links of the free block starting at `pfn`.
    fn get(&mut self, pfn: u64) -> (u64, u64);
    /// Writes the links of the free block starting at `pfn`.
    fn set(&mut self, pfn: u64, next: u64, prev: u64);
}

/// A buddy allocator for page frames `0..state.len()`, with block orders
/// `0..NORDER`.
pub struct Buddy<'a, L: Links, const NORDER: usize> {
    state: &'a mut [u8],
    heads: [u64; NORDER],
    nfree: u64,
    links: L,
}

impl<'a, L: Links, const NORDER: usize> Buddy<'a, L, NORDER> {
    /// Creates an allocator with nothing free. `state` holds one byte per
    /// page frame and must be zeroed.
    pub fn new(state: &'a mut [u8], links: L) -> Self {
        assert!(NORDER > 0 && NORDER < FREE as usize);
        assert!(
            state.iter().all(|&s| s == 0),
            "buddy state must start zeroed"
        );
        Self {
            state,
            heads: [NIL; NORDER],
            nfree: 0,
            links,
        }
    }

    /// Number of free pages.
    pub fn free_pages(&self) -> u64 {
        self.nfree
    }

    /// Number of free blocks of each order.
    pub fn free_blocks(&mut self, order: usize) -> usize {
        let mut n = 0;
        let mut pfn = self.heads[order];
        while pfn != NIL {
            n += 1;
            pfn = self.links.get(pfn).0;
        }
        n
    }

    /// Hands pages `[start, end)` to the allocator, as the largest aligned
    /// blocks that fit.
    pub fn add_range(&mut self, start: u64, end: u64) {
        let end = end.min(self.state.len() as u64);
        let mut pfn = start;
        while pfn < end {
            let mut order = NORDER - 1;
            while !pfn.is_multiple_of(1 << order) || pfn + (1 << order) > end {
                order -= 1;
            }
            self.free(pfn, order);
            pfn += 1 << order;
        }
    }

    /// Allocates a block of `2^order` pages, aligned to its size.
    pub fn alloc(&mut self, order: usize) -> Option<u64> {
        let mut o = (order..NORDER).find(|&o| self.heads[o] != NIL)?;
        let pfn = self.heads[o];
        self.remove(pfn, o);
        // Split, keeping the lower half and freeing the upper ones.
        while o > order {
            o -= 1;
            self.push(pfn + (1 << o), o);
        }
        Some(pfn)
    }

    /// Returns a block obtained from [`Buddy::alloc`] with the same order,
    /// merging it with its buddies.
    ///
    /// # Panics
    ///
    /// If the block is misaligned, out of range, or already free.
    pub fn free(&mut self, mut pfn: u64, mut order: usize) {
        assert!(order < NORDER, "buddy: bad order {order}");
        assert!(
            pfn.is_multiple_of(1 << order),
            "buddy: misaligned block {pfn:#x}/{order}"
        );
        assert!(
            pfn + (1 << order) <= self.state.len() as u64,
            "buddy: block {pfn:#x} out of range"
        );
        assert!(
            self.state[pfn as usize] & FREE == 0,
            "buddy: double free of {pfn:#x}"
        );
        while order + 1 < NORDER {
            let buddy = pfn ^ (1 << order);
            if buddy as usize >= self.state.len()
                || self.state[buddy as usize] != FREE | order as u8
            {
                break;
            }
            self.remove(buddy, order);
            pfn = pfn.min(buddy);
            order += 1;
        }
        self.push(pfn, order);
    }

    fn push(&mut self, pfn: u64, order: usize) {
        let head = self.heads[order];
        self.links.set(pfn, head, NIL);
        if head != NIL {
            let (next, _) = self.links.get(head);
            self.links.set(head, next, pfn);
        }
        self.heads[order] = pfn;
        self.state[pfn as usize] = FREE | order as u8;
        self.nfree += 1 << order;
    }

    fn remove(&mut self, pfn: u64, order: usize) {
        let (next, prev) = self.links.get(pfn);
        if prev == NIL {
            self.heads[order] = next;
        } else {
            let (_, pprev) = self.links.get(prev);
            self.links.set(prev, next, pprev);
        }
        if next != NIL {
            let (nnext, _) = self.links.get(next);
            self.links.set(next, nnext, prev);
        }
        self.state[pfn as usize] = 0;
        self.nfree -= 1 << order;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct VecLinks(Vec<(u64, u64)>);

    impl Links for VecLinks {
        fn get(&mut self, pfn: u64) -> (u64, u64) {
            self.0[pfn as usize]
        }
        fn set(&mut self, pfn: u64, next: u64, prev: u64) {
            self.0[pfn as usize] = (next, prev);
        }
    }

    fn buddy(state: &mut [u8]) -> Buddy<'_, VecLinks, 4> {
        let n = state.len();
        Buddy::new(state, VecLinks(vec![(NIL, NIL); n]))
    }

    #[test]
    fn add_range_uses_largest_aligned_blocks() {
        let mut st = [0u8; 32];
        let mut b = buddy(&mut st);
        b.add_range(1, 19); // 1, 2-3, 4-7, 8-15, 16-17, 18
        assert_eq!(b.free_pages(), 18);
        assert_eq!(b.free_blocks(3), 1);
        assert_eq!(b.free_blocks(2), 1);
        assert_eq!(b.free_blocks(1), 2);
        assert_eq!(b.free_blocks(0), 2);
    }

    #[test]
    fn split_and_merge_back() {
        let mut st = [0u8; 16];
        let mut b = buddy(&mut st);
        b.add_range(0, 16);
        assert_eq!(b.free_blocks(3), 2);
        let pages: Vec<u64> = (0..16).map(|_| b.alloc(0).unwrap()).collect();
        assert_eq!(b.free_pages(), 0);
        assert!(b.alloc(0).is_none());
        let mut sorted = pages.clone();
        sorted.sort();
        assert_eq!(sorted, (0..16).collect::<Vec<_>>());
        for p in pages.into_iter().rev() {
            b.free(p, 0);
        }
        assert_eq!(b.free_pages(), 16);
        assert_eq!(b.free_blocks(3), 2);
        assert_eq!(b.free_blocks(0), 0);
    }

    #[test]
    fn blocks_are_aligned_and_disjoint() {
        let mut st = [0u8; 64];
        let mut b = buddy(&mut st);
        b.add_range(3, 61);
        let mut used = [false; 64];
        while let Some(p) = b.alloc(2) {
            assert_eq!(p % 4, 0);
            for i in p..p + 4 {
                assert!(!used[i as usize]);
                used[i as usize] = true;
            }
        }
        while let Some(p) = b.alloc(0) {
            assert!(!used[p as usize]);
            used[p as usize] = true;
        }
        assert_eq!(used.iter().filter(|&&u| u).count(), 58);
    }

    #[test]
    #[should_panic(expected = "double free")]
    fn double_free_panics() {
        let mut st = [0u8; 4];
        let mut b = buddy(&mut st);
        b.add_range(0, 1);
        let p = b.alloc(0).unwrap();
        b.free(p, 0);
        b.free(p, 0);
    }
}

/// Proofs (`make kani`) on a small zone, split in two so each fits in a
/// few hundred megabytes: (a) for every range and every order, one block
/// is aligned and in range, and freeing it coalesces back to exactly what
/// `add_range` built; (b) on the whole zone, any two requests get disjoint
/// blocks, no page is lost, and freeing both coalesces back.
#[cfg(kani)]
mod verification {
    use super::*;

    const NPAGES: usize = 4;
    const NORDER: usize = 3;

    struct ArrayLinks([(u64, u64); NPAGES]);

    impl Links for ArrayLinks {
        fn get(&mut self, pfn: u64) -> (u64, u64) {
            self.0[pfn as usize]
        }
        fn set(&mut self, pfn: u64, next: u64, prev: u64) {
            self.0[pfn as usize] = (next, prev);
        }
    }

    fn zone(state: &mut [u8; NPAGES], start: u64, end: u64) -> Buddy<'_, ArrayLinks, NORDER> {
        let mut b = Buddy::new(state, ArrayLinks([(NIL, NIL); NPAGES]));
        b.add_range(start, end);
        b
    }

    fn assert_same_free_lists(b: &mut Buddy<'_, ArrayLinks, NORDER>, start: u64, end: u64) {
        let mut st = [0u8; NPAGES];
        let mut fresh = zone(&mut st, start, end);
        assert_eq!(b.free_pages(), fresh.free_pages());
        for o in 0..NORDER {
            assert_eq!(b.free_blocks(o), fresh.free_blocks(o), "not coalesced");
        }
    }

    #[kani::proof]
    #[kani::unwind(5)]
    fn one_block_any_range() {
        let start: u64 = kani::any_where(|&s| s <= NPAGES as u64);
        let end: u64 = kani::any_where(|&e| e >= start && e <= NPAGES as u64);
        let mut st = [0u8; NPAGES];
        let mut b = zone(&mut st, start, end);
        assert_eq!(b.free_pages(), end - start);
        let order: usize = kani::any_where(|&o| o < NORDER);
        if let Some(pfn) = b.alloc(order) {
            let n = 1u64 << order;
            assert!(pfn.is_multiple_of(n), "misaligned block");
            assert!(pfn >= start && pfn + n <= end, "block outside the zone");
            assert_eq!(b.free_pages() + n, end - start, "pages lost");
            b.free(pfn, order);
        }
        assert_same_free_lists(&mut b, start, end);
    }

    #[kani::proof]
    #[kani::unwind(5)]
    fn two_blocks_are_disjoint() {
        let end = NPAGES as u64;
        let mut st = [0u8; NPAGES];
        let mut b = zone(&mut st, 0, end);
        let (o1, o2): (usize, usize) = (kani::any_where(|&o| o < NORDER), kani::any_where(|&o| o < NORDER));
        let p1 = b.alloc(o1);
        let p2 = b.alloc(o2);
        let used = p1.map_or(0, |_| 1u64 << o1) + p2.map_or(0, |_| 1u64 << o2);
        assert_eq!(b.free_pages() + used, end, "pages lost");
        if let (Some(a), Some(c)) = (p1, p2) {
            assert!(a + (1 << o1) <= c || c + (1 << o2) <= a, "blocks overlap");
        }
        if let Some(a) = p1 {
            b.free(a, o1);
        }
        if let Some(c) = p2 {
            b.free(c, o2);
        }
        assert_same_free_lists(&mut b, 0, end);
    }
}
