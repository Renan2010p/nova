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

//! Physical memory ranges (in the spirit of FreeBSD's `sys/physmem.h`).
//!
//! Each architecture reads its firmware's memory map (SMAP, EFI, device
//! tree, ...) and feeds the usable ranges into a [`PhysMap`], which keeps
//! them sorted and coalesced. The algorithm lives here, in MI code, so it
//! is written once and unit-tested on the host.

/// A range of physical addresses, `[start, end)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PhysRange {
    /// First byte.
    pub start: u64,
    /// One past the last byte.
    pub end: u64,
}

impl PhysRange {
    /// The empty range.
    pub const EMPTY: Self = Self { start: 0, end: 0 };

    /// Size in bytes.
    pub const fn len(&self) -> u64 {
        self.end.saturating_sub(self.start)
    }

    /// True if the range holds no bytes.
    pub const fn is_empty(&self) -> bool {
        self.end <= self.start
    }
}

/// A fixed-capacity, sorted set of non-overlapping, non-adjacent ranges.
/// Needs no allocator, so it works before the VM system exists.
#[derive(Clone, Debug)]
pub struct PhysMap<const N: usize> {
    ranges: [PhysRange; N],
    n: usize,
    overflowed: bool,
}

impl<const N: usize> PhysMap<N> {
    /// An empty map.
    pub const fn new() -> Self {
        Self {
            ranges: [PhysRange::EMPTY; N],
            n: 0,
            overflowed: false,
        }
    }

    /// Adds `[start, start + len)`, merging it with any range it overlaps
    /// or touches (`add_physmap_entry`). Returns false, and remembers it,
    /// if the range was dropped for lack of space or because it wraps.
    pub fn add(&mut self, start: u64, len: u64) -> bool {
        if len == 0 {
            return true;
        }
        let Some(end) = start.checked_add(len) else {
            self.overflowed = true;
            return false;
        };
        let r = &mut self.ranges;
        let i = r[..self.n].partition_point(|e| e.start <= start);
        let idx = if i > 0 && r[i - 1].end >= start {
            r[i - 1].end = r[i - 1].end.max(end);
            i - 1
        } else {
            if self.n == N {
                self.overflowed = true;
                return false;
            }
            r.copy_within(i..self.n, i + 1);
            r[i] = PhysRange { start, end };
            self.n += 1;
            i
        };
        // Swallow the following ranges that now overlap or touch.
        while idx + 1 < self.n && r[idx + 1].start <= r[idx].end {
            r[idx].end = r[idx].end.max(r[idx + 1].end);
            r.copy_within(idx + 2..self.n, idx + 1);
            self.n -= 1;
        }
        true
    }

    /// The ranges, sorted by address.
    pub fn as_slice(&self) -> &[PhysRange] {
        &self.ranges[..self.n]
    }

    /// True if some range could not be recorded.
    pub fn overflowed(&self) -> bool {
        self.overflowed
    }
}

impl<const N: usize> Default for PhysMap<N> {
    fn default() -> Self {
        Self::new()
    }
}

/// Total bytes covered by `ranges`.
pub fn total(ranges: &[PhysRange]) -> u64 {
    ranges.iter().map(PhysRange::len).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(start: u64, end: u64) -> PhysRange {
        PhysRange { start, end }
    }

    #[test]
    fn sorts_and_coalesces() {
        let mut m = PhysMap::<8>::new();
        m.add(0x10_0000, 0x10_0000); // [1M, 2M)
        m.add(0, 0x9_f000); // [0, 636K)
        m.add(0x20_0000, 0x20_0000); // touches the first: [1M, 4M)
        m.add(0x80_0000, 0x10_0000);
        assert_eq!(
            m.as_slice(),
            [
                r(0, 0x9_f000),
                r(0x10_0000, 0x40_0000),
                r(0x80_0000, 0x90_0000)
            ]
        );
        assert_eq!(total(m.as_slice()), 0x9_f000 + 0x30_0000 + 0x10_0000);
    }

    #[test]
    fn a_big_range_swallows_many() {
        let mut m = PhysMap::<8>::new();
        for i in 0..5 {
            m.add(i * 0x1000 * 2, 0x1000);
        }
        assert_eq!(m.as_slice().len(), 5);
        m.add(0x500, 0x10_0000);
        assert_eq!(m.as_slice(), [r(0, 0x10_0500)]);
    }

    #[test]
    fn overflow_and_wrap_are_reported() {
        let mut m = PhysMap::<2>::new();
        assert!(m.add(0, 1));
        assert!(m.add(10, 1));
        assert!(!m.add(20, 1));
        assert!(m.overflowed());
        let mut m = PhysMap::<2>::new();
        assert!(!m.add(u64::MAX, 2));
        assert!(m.overflowed());
        assert!(m.add(5, 0));
    }
}
