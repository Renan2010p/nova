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

//! The boot framebuffer, as a safe drawing surface (the mechanism under
//! FreeBSD's `vt_efifb`/`vt_vbefb`): what console drivers in `sys/dev`
//! draw on. Every operation is clipped to the screen, so safe code can
//! never write outside it.
//!
//! The framebuffer is reached through the direct map. Firmware maps it
//! uncached or write-combining with the MTRRs, which take precedence over
//! the direct map's write-back type.
//!
//! TODO(fb): framebuffers above the direct map; write-combining through
//! PAT.

use mdi::DirectMap;

use crate::sync::{MtxSpin, OnceLock};
use crate::{FbInfo, Md};

/// A glyph: `width` x `height` pixels, one bit each, rows of
/// `(width + 7) / 8` bytes, most significant bit leftmost (FreeBSD's `vfnt`
/// layout).
#[derive(Clone, Copy, Debug)]
pub struct Glyph<'a> {
    /// The bits.
    pub bits: &'a [u8],
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
}

/// A pixel value, already in the framebuffer's format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pixel(u32);

/// The framebuffer the firmware left on.
pub struct Framebuffer {
    /// Direct-map address of the first pixel.
    base: usize,
    width: u32,
    height: u32,
    /// Pixels per line.
    stride: u32,
    /// Size in bytes, from the firmware: no access goes beyond it.
    size: u64,
    masks: [u32; 3],
    /// Serializes drawing (several CPUs may print).
    lock: MtxSpin<()>,
}

static FB: OnceLock<Framebuffer> = OnceLock::new();

/// Takes the framebuffer the loader described, if it is in the direct map.
pub(crate) fn init(fb: Option<FbInfo>) {
    let Some(fb) = fb else { return };
    let need = u64::from(fb.stride) * u64::from(fb.height) * 4;
    let end = fb.paddr.checked_add(fb.size);
    if need > fb.size || end.is_none_or(|e| e > Md::dmap_limit()) {
        return;
    }
    let _ = FB.set(Framebuffer {
        base: Md::phys_to_dmap(fb.paddr),
        width: fb.width,
        height: fb.height,
        stride: fb.stride,
        size: fb.size,
        masks: [fb.red_mask, fb.green_mask, fb.blue_mask],
        lock: MtxSpin::new(()),
    });
}

/// The framebuffer, if there is one.
pub fn framebuffer() -> Option<&'static Framebuffer> {
    FB.get()
}

/// `v` (0-255) scaled into the bits of `mask`.
fn channel(v: u8, mask: u32) -> u32 {
    if mask == 0 {
        return 0;
    }
    let shift = mask.trailing_zeros();
    let bits = mask.count_ones().min(8);
    ((u32::from(v) >> (8 - bits)) << shift) & mask
}

impl Framebuffer {
    /// Width in pixels.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Height in pixels.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// The pixel value of a color.
    pub fn rgb(&self, r: u8, g: u8, b: u8) -> Pixel {
        let [rm, gm, bm] = self.masks;
        Pixel(channel(r, rm) | channel(g, gm) | channel(b, bm))
    }

    /// Address of pixel (`x`, `y`), which must be on the screen.
    fn at(&self, x: u32, y: u32) -> *mut u32 {
        let off = (u64::from(y) * u64::from(self.stride) + u64::from(x)) * 4;
        debug_assert!(x < self.width && y < self.height && off + 4 <= self.size);
        // Inside the framebuffer (`init` checked `stride * height * 4 <=
        // size`), hence inside the direct map.
        core::ptr::with_exposed_provenance_mut::<u32>(self.base + libkern::conv::usize_from_u64(off))
    }

    fn put(&self, x: u32, y: u32, p: Pixel) {
        // SAFETY: (x, y) is on the screen (every caller clips), so the
        // address is inside the framebuffer, which the firmware gave the
        // kernel; it is device memory, written with volatile stores only.
        unsafe { self.at(x, y).write_volatile(p.0) }
    }

    fn get(&self, x: u32, y: u32) -> Pixel {
        // SAFETY: as in `put`.
        Pixel(unsafe { self.at(x, y).read_volatile() })
    }

    /// Fills a rectangle, clipped to the screen.
    pub fn fill(&self, x: u32, y: u32, w: u32, h: u32, p: Pixel) {
        let _g = self.lock.lock();
        let (x1, y1) = (x.saturating_add(w).min(self.width), y.saturating_add(h).min(self.height));
        for yy in y..y1 {
            for xx in x..x1 {
                self.put(xx, yy, p);
            }
        }
    }

    /// Draws `glyph` at (`x`, `y`) with `fg` for set bits and `bg` for clear
    /// ones. Clipped.
    pub fn draw_glyph(&self, x: u32, y: u32, glyph: Glyph<'_>, fg: Pixel, bg: Pixel) {
        let _g = self.lock.lock();
        let (w, h) = (glyph.width, glyph.height);
        let bpr = libkern::conv::usize_from_u32(w.div_ceil(8));
        for row in 0..h {
            let yy = y.saturating_add(row);
            if yy >= self.height {
                break;
            }
            let line = glyph
                .bits
                .get(libkern::conv::usize_from_u32(row) * bpr..)
                .and_then(|l| l.get(..bpr))
                .unwrap_or(&[]);
            for col in 0..w {
                let xx = x.saturating_add(col);
                if xx >= self.width {
                    break;
                }
                let byte = line.get(libkern::conv::usize_from_u32(col / 8)).copied().unwrap_or(0);
                let set = byte & (0x80 >> (col % 8)) != 0;
                self.put(xx, yy, if set { fg } else { bg });
            }
        }
    }

    /// Moves everything up by `n` pixel rows and fills the freed rows at
    /// the bottom with `bg` (scrolling).
    pub fn scroll_up(&self, n: u32, bg: Pixel) {
        let _g = self.lock.lock();
        let n = n.min(self.height);
        for y in 0..self.height - n {
            for x in 0..self.width {
                let p = self.get(x, y + n);
                self.put(x, y, p);
            }
        }
        for y in self.height - n..self.height {
            for x in 0..self.width {
                self.put(x, y, bg);
            }
        }
    }
}
