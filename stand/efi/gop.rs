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

//! The framebuffer (`EFI_GRAPHICS_OUTPUT_PROTOCOL`), described for the
//! kernel (`MODINFOMD_EFI_FB`).

#![allow(unsafe_code)]

use core::ffi::c_void;

use crate::table::{Guid, boot_services};
use crate::Error;

const GOP_GUID: Guid = Guid(0x9042_a9de, 0x23dc, 0x4a38, [0x96, 0xfb, 0x7a, 0xde, 0xd0, 0x80, 0x51, 0x6a]);

#[repr(C)]
struct Gop {
    query_mode: *mut c_void,
    set_mode: *mut c_void,
    blt: *mut c_void,
    mode: *const GopMode,
}

#[repr(C)]
struct GopMode {
    max_mode: u32,
    mode: u32,
    info: *const ModeInfo,
    size_of_info: usize,
    frame_buffer_base: u64,
    frame_buffer_size: usize,
}

#[repr(C)]
struct ModeInfo {
    version: u32,
    horizontal_resolution: u32,
    vertical_resolution: u32,
    pixel_format: u32,
    red_mask: u32,
    green_mask: u32,
    blue_mask: u32,
    reserved_mask: u32,
    pixels_per_scan_line: u32,
}

/// The current graphics mode, as FreeBSD's `struct efi_fb`.
#[derive(Clone, Copy, Debug)]
pub struct Framebuffer {
    /// Physical address.
    pub addr: u64,
    /// Size in bytes.
    pub size: u64,
    /// Height in pixels.
    pub height: u32,
    /// Width in pixels.
    pub width: u32,
    /// Pixels per line.
    pub stride: u32,
    /// Masks of red, green, blue and the unused bits.
    pub masks: [u32; 4],
}

/// The framebuffer, if the firmware has graphics output with one.
pub fn framebuffer() -> Result<Framebuffer, Error> {
    let bs = boot_services()?;
    let mut p: *mut c_void = core::ptr::null_mut();
    // SAFETY: a boot service, with an out pointer to a local.
    unsafe { (bs.locate_protocol)(&GOP_GUID, core::ptr::null_mut(), &mut p) }.ok()?;
    // SAFETY: the firmware's GOP interface and its mode, alive while the
    // boot services are.
    let (mode, info) = unsafe {
        let gop = p.cast::<Gop>().as_ref().ok_or(Error::NotFound)?;
        let mode = gop.mode.as_ref().ok_or(Error::NotFound)?;
        (mode, mode.info.as_ref().ok_or(Error::NotFound)?)
    };
    // PixelRedGreenBlueReserved8BitPerColor, PixelBlueGreenRed..., BitMask.
    let masks = match info.pixel_format {
        0 => [0x0000_00ff, 0x0000_ff00, 0x00ff_0000, 0xff00_0000],
        1 => [0x00ff_0000, 0x0000_ff00, 0x0000_00ff, 0xff00_0000],
        2 => [info.red_mask, info.green_mask, info.blue_mask, info.reserved_mask],
        _ => return Err(Error::NotFound), // BltOnly: no framebuffer
    };
    Ok(Framebuffer {
        addr: mode.frame_buffer_base,
        size: libkern::conv::u64_from_usize(mode.frame_buffer_size),
        height: info.vertical_resolution,
        width: info.horizontal_resolution,
        stride: info.pixels_per_scan_line,
        masks,
    })
}

/// The framebuffer as a drawing surface: 32-bit pixels, every access
/// clipped to the screen. Valid while the firmware's graphics mode is (the
/// loader draws its menu before leaving the boot services).
pub struct Surface {
    fb: Framebuffer,
}

impl Surface {
    /// The current graphics mode's framebuffer, if any.
    pub fn new() -> Option<Surface> {
        framebuffer().ok().filter(|f| f.size >= u64::from(f.stride) * u64::from(f.height) * 4).map(|fb| Surface { fb })
    }

    /// Width and height in pixels.
    pub fn size(&self) -> (u32, u32) {
        (self.fb.width, self.fb.height)
    }

    /// The pixel value of a colour.
    pub fn rgb(&self, r: u8, g: u8, b: u8) -> u32 {
        let ch = |v: u8, m: u32| if m == 0 { 0 } else { (u32::from(v) << m.trailing_zeros()) & m };
        ch(r, self.fb.masks[0]) | ch(g, self.fb.masks[1]) | ch(b, self.fb.masks[2])
    }

    /// Fills a rectangle, clipped.
    pub fn fill(&self, x: u32, y: u32, w: u32, h: u32, px: u32) {
        let x1 = x.saturating_add(w).min(self.fb.width);
        let y1 = y.saturating_add(h).min(self.fb.height);
        for yy in y..y1 {
            for xx in x..x1 {
                self.put(xx, yy, px);
            }
        }
    }

    /// Sets one pixel, if on the screen.
    pub fn put(&self, x: u32, y: u32, px: u32) {
        if x >= self.fb.width || y >= self.fb.height {
            return;
        }
        let off = (u64::from(y) * u64::from(self.fb.stride) + u64::from(x)) * 4;
        let addr = libkern::conv::usize_from_u64(self.fb.addr + off);
        // SAFETY: (x, y) is on the screen, so `addr` is inside the
        // framebuffer the firmware described (`new` checked its size); it is
        // device memory, written with a volatile store only.
        unsafe { core::ptr::with_exposed_provenance_mut::<u32>(addr).write_volatile(px) };
    }
}
