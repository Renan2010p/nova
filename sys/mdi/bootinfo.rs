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

/// The boot environment, in machine-independent terms.
#[derive(Clone, Copy, Debug)]
pub struct BootInfo<'a> {
    /// Firmware the loader ran on.
    pub firmware: Firmware,
    /// Usable RAM, sorted and coalesced. Includes the memory occupied by
    /// the kernel and the loader's data; see `kernel`.
    pub physmem: &'a [PhysRange],
    /// Physical memory occupied by the kernel image, the modules and the
    /// boot metadata. Must not be handed to the page allocator.
    pub kernel: PhysRange,
    /// `boothowto` flags, see `sys::reboot`.
    pub howto: u32,
    /// The loader's environment: `name=value` strings, each NUL-terminated,
    /// ending with an empty string. May run past that terminator; the
    /// consumer (`kern::kern_environment`) finds it.
    pub envp: &'a [u8],
    /// Raw preload metadata, for MI consumers (preloaded modules,
    /// `md_image`, symbol table, ...).
    pub preload: Preload<'a>,
}
