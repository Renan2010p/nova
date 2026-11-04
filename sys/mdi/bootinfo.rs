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

//! What the MD bootstrap hands to `mi_startup`.
//!
//! Every architecture translates its own boot environment (BIOS SMAP, EFI
//! memory map, device tree, ...) into this one MI description, so the MI
//! kernel never parses firmware tables.

use sys::linker::Preload;
use sys::physmem::PhysRange;

/// Firmware the machine booted from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Firmware {
    /// Legacy PC BIOS.
    Bios,
    /// UEFI.
    Uefi,
    /// Could not be determined (e.g. a hypervisor's userboot loader).
    Unknown,
}

impl Firmware {
    /// Human-readable name.
    pub const fn name(self) -> &'static str {
        match self {
            Firmware::Bios => "BIOS",
            Firmware::Uefi => "UEFI",
            Firmware::Unknown => "unknown",
        }
    }
}

/// A linear framebuffer the firmware set up (UEFI GOP, VESA), in
/// machine-independent terms: 32-bit pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FbInfo {
    /// Physical address of the first pixel.
    pub paddr: u64,
    /// Size in bytes.
    pub size: u64,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Pixels from one line to the next (at least `width`).
    pub stride: u32,
    /// Bits of a pixel holding red, green and blue.
    pub red_mask: u32,
    /// See `red_mask`.
    pub green_mask: u32,
    /// See `red_mask`.
    pub blue_mask: u32,
}

/// The boot environment, in machine-independent terms.
#[derive(Clone, Copy, Debug)]
pub struct BootInfo<'a> {
    /// Firmware the loader ran on.
    pub firmware: Firmware,
    /// All usable RAM reported by the firmware, sorted and coalesced
    /// (for reporting: "real memory").
    pub physmem: &'a [PhysRange],
    /// The part of `physmem` the page allocator may use (FreeBSD's
    /// `phys_avail`): without the kernel, the loader's data, the early page
    /// tables, memory the MD layer reserves (e.g. low memory for the AP
    /// trampoline), and anything outside the direct map.
    pub physavail: &'a [PhysRange],
    /// Physical memory occupied by the kernel image, the modules, the boot
    /// metadata and the early page tables.
    pub kernel: PhysRange,
    /// `boothowto` flags, see `sys::reboot`.
    pub howto: u32,
    /// The loader's environment: `name=value` strings, each NUL-terminated,
    /// ending with an empty string. May run past that terminator; the
    /// consumer (`kern::kern_environment`) finds it.
    pub envp: &'a [u8],
    /// What the loader placed in memory after the kernel image (preloaded
    /// files, symbols, metadata), starting at physical `loaded_base`, so
    /// that MI code can read a preloaded file from its `MODINFO_ADDR` and
    /// `MODINFO_SIZE`.
    pub loaded: &'static [u8],
    /// Physical address of `loaded[0]`.
    pub loaded_base: u64,
    /// What to subtract from an address in the preload metadata (the
    /// loader stores some as kernel virtual addresses) to get its physical
    /// address (FreeBSD's `preload_addr_relocate`, negated).
    pub preload_addr_relocate: u64,
    /// The framebuffer, if the firmware set one up.
    pub framebuffer: Option<FbInfo>,
    /// The kernel symbol table preloaded by the loader, `[SSYM, ESYM)`, in
    /// the layout `sys::elf::KSymTab` reads; empty if absent.
    pub ksyms: &'static [u8],
    /// Raw preload metadata, for MI consumers (preloaded modules,
    /// `md_image`, symbol table, ...).
    pub preload: Preload<'a>,
}
