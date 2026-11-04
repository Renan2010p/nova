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

//! Bounds-checked reads of native-endian integers from byte slices.
//!
//! Firmware and loader data structures are parsed with these instead of
//! casting pointers to `#[repr(C)]` structs: a truncated or corrupted
//! table yields `None` rather than undefined behaviour.

/// Reads a native-endian `u32` at `off`, or `None` if out of bounds.
pub fn read_u32(buf: &[u8], off: usize) -> Option<u32> {
    let b = buf.get(off..off.checked_add(4)?)?;
    Some(u32::from_ne_bytes(b.try_into().ok()?))
}

/// Reads a native-endian `u64` at `off`, or `None` if out of bounds.
pub fn read_u64(buf: &[u8], off: usize) -> Option<u64> {
    let b = buf.get(off..off.checked_add(8)?)?;
    Some(u64::from_ne_bytes(b.try_into().ok()?))
}

/// Rounds `v` up to a multiple of `align`, which must be a power of two.
/// Returns `None` on overflow.
pub const fn roundup2(v: usize, align: usize) -> Option<usize> {
    match v.checked_add(align - 1) {
        Some(x) => Some(x & !(align - 1)),
        None => None,
    }
}

#[cfg(test)]
#[allow(clippy::as_conversions, reason = "host test fixtures, not kernel code")]
mod tests {
    use super::*;

    #[test]
    fn reads_are_bounds_checked() {
        let b = 0x1122_3344u32.to_ne_bytes();
        assert_eq!(read_u32(&b, 0), Some(0x1122_3344));
        assert_eq!(read_u32(&b, 1), None);
        assert_eq!(read_u64(&b, 0), None);
        assert_eq!(read_u32(&b, usize::MAX), None);
    }

    #[test]
    fn roundup() {
        assert_eq!(roundup2(0, 8), Some(0));
        assert_eq!(roundup2(9, 8), Some(16));
        assert_eq!(roundup2(16, 8), Some(16));
        assert_eq!(roundup2(usize::MAX, 8), None);
    }
}
