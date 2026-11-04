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

//! Console fonts in FreeBSD's `vt(4)` layout (`sys/sys/font.h`): a
//! bitmap of glyphs and maps from code points to glyphs, one per style.
//! The loader hands the kernel such a font (`MODINFOMD_FONT`, a
//! `struct font_info` followed by the maps and the bitmap) when it is not
//! the built-in one.

use crate::bytes::{read_u16, read_u32};
use libkern::conv::usize_from_u32;

/// Map styles (`VFNT_MAP_*`).
pub const VFNT_MAPS: usize = 4;
/// Normal weight.
pub const VFNT_MAP_NORMAL: usize = 0;
/// Bold.
pub const VFNT_MAP_BOLD: usize = 2;

/// Size of a `struct vfnt_map` record: `src` u32, `dst` u16, `len` u16.
const MAP_RECORD: usize = 8;
/// Size of `struct font_info`.
const FONT_INFO_SIZE: usize = 32;

/// A console font.
#[derive(Clone, Copy, Debug)]
pub struct Font<'a> {
    /// Glyph width in pixels.
    pub width: u32,
    /// Glyph height in pixels.
    pub height: u32,
    bitmap: &'a [u8],
    maps: [&'a [u8]; VFNT_MAPS],
}

impl<'a> Font<'a> {
    /// A font from its parts. `None` if a map is not made of whole records
    /// or the bitmap has no glyph.
    pub fn new(width: u32, height: u32, bitmap: &'a [u8], maps: [&'a [u8]; VFNT_MAPS]) -> Option<Self> {
        let f = Font { width, height, bitmap, maps };
        let glyph = f.glyph_size()?;
        (width > 0 && height > 0 && bitmap.len() >= glyph && maps.iter().all(|m| m.len() % MAP_RECORD == 0))
            .then_some(f)
    }

    /// Reads the loader's font (`parse_font_info`): the header, whose
    /// checksum makes its fields add up to zero, then each map rounded up
    /// to 8 bytes, then the bitmap.
    pub fn from_font_info(b: &'a [u8]) -> Option<Self> {
        let checksum = read_u32(b, 0)?;
        let width = read_u32(b, 4)?;
        let height = read_u32(b, 8)?;
        let bitmap_size = read_u32(b, 12)?;
        let mut counts = [0u32; VFNT_MAPS];
        let mut sum = width.wrapping_add(height).wrapping_add(bitmap_size);
        for (i, c) in counts.iter_mut().enumerate() {
            *c = read_u32(b, 16 + 4 * i)?;
            sum = sum.wrapping_add(*c);
        }
        if checksum.wrapping_add(sum) != 0 {
            return None;
        }
        let mut off = FONT_INFO_SIZE;
        let mut maps: [&[u8]; VFNT_MAPS] = [&[]; VFNT_MAPS];
        for (map, &count) in maps.iter_mut().zip(&counts) {
            if count == 0 {
                continue;
            }
            let len = usize_from_u32(count).checked_mul(MAP_RECORD)?;
            *map = b.get(off..off.checked_add(len)?)?;
            off = off.checked_add(len)?.checked_next_multiple_of(8)?;
        }
        let bitmap = b.get(off..off.checked_add(usize_from_u32(bitmap_size))?)?;
        Font::new(width, height, bitmap, maps)
    }

    /// Bytes of one glyph: rows of `(width + 7) / 8` bytes.
    pub fn glyph_size(&self) -> Option<usize> {
        usize_from_u32(self.width.div_ceil(8)).checked_mul(usize_from_u32(self.height))
    }

    /// The glyph index of `c` in map `m` (`vtfont_lookup`): the record
    /// with `src <= c <= src + len` maps it to `dst + (c - src)`.
    fn lookup(&self, m: usize, c: u32) -> Option<usize> {
        let map = self.maps.get(m)?;
        let n = map.len() / MAP_RECORD;
        let (mut lo, mut hi) = (0, n);
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let rec = mid * MAP_RECORD;
            let src = read_u32(map, rec)?;
            let dst = read_u16(map, rec + 4)?;
            let len = read_u16(map, rec + 6)?;
            if c < src {
                hi = mid;
            } else if c > src.saturating_add(u32::from(len)) {
                lo = mid + 1;
            } else {
                return Some(usize::from(dst) + usize_from_u32(c - src));
            }
        }
        None
    }

    /// The glyph for `c`: from the bold map if `bold` and it has one, else
    /// the normal map, else glyph 0 (the "missing" box).
    pub fn glyph(&self, c: char, bold: bool) -> &'a [u8] {
        let c = u32::from(c);
        let index = (if bold { self.lookup(VFNT_MAP_BOLD, c) } else { None })
            .or_else(|| self.lookup(VFNT_MAP_NORMAL, c))
            .unwrap_or(0);
        let size = self.glyph_size().unwrap_or(0);
        index
            .checked_mul(size)
            .and_then(|at| self.bitmap.get(at..at.checked_add(size)?))
            .or_else(|| self.bitmap.get(..size))
            .unwrap_or(&[])
    }
}

#[cfg(test)]
#[allow(clippy::as_conversions, reason = "host test fixtures, not kernel code")]
mod tests {
    use super::*;

    fn rec(src: u32, dst: u16, len: u16) -> Vec<u8> {
        let mut v = src.to_le_bytes().to_vec();
        v.extend_from_slice(&dst.to_le_bytes());
        v.extend_from_slice(&len.to_le_bytes());
        v
    }

    #[test]
    fn lookup_ranges() {
        let bitmap: Vec<u8> = (0..4u8).flat_map(|g| [g; 16]).collect();
        let normal = [rec(0x20, 1, 1), rec(0x41, 3, 0)].concat();
        let f = Font::new(8, 16, &bitmap, [&normal, &[], &[], &[]]).unwrap();
        assert_eq!(f.glyph(' ', false)[0], 1);
        assert_eq!(f.glyph('!', false)[0], 2);
        assert_eq!(f.glyph('A', true)[0], 3); // no bold map: normal
        assert_eq!(f.glyph('Z', false)[0], 0); // missing
    }

    #[test]
    fn font_info_round_trip() {
        let bitmap = [0xaau8; 32];
        let normal = rec(0x20, 1, 0);
        let mut b = Vec::new();
        let (w, h, size, count) = (8u32, 16u32, 32u32, 1u32);
        let checksum = 0u32.wrapping_sub(w + h + size + count);
        for x in [checksum, w, h, size, count, 0, 0, 0] {
            b.extend_from_slice(&x.to_le_bytes());
        }
        b.extend_from_slice(&normal);
        b.extend_from_slice(&bitmap);
        let f = Font::from_font_info(&b).unwrap();
        assert_eq!((f.width, f.height), (8, 16));
        assert_eq!(f.glyph(' ', false), &bitmap[16..32]);
        b[0] ^= 1;
        assert!(Font::from_font_info(&b).is_none(), "a bad checksum is refused");
    }
}
