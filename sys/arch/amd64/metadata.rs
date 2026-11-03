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

//! Machine-dependent preload record types and their payloads (FreeBSD's
//! `x86/include/metadata.h` and `amd64/include/pc/bios.h`).

/// BIOS INT 15h/E820 memory map: an array of [`SMAP_ENTRY_SIZE`] entries.
pub const MODINFOMD_SMAP: u32 = 0x1001;
/// Extended attributes for the SMAP entries.
pub const MODINFOMD_SMAP_XATTR: u32 = 0x1002;
/// Device tree blob pointer.
pub const MODINFOMD_DTBP: u32 = 0x1003;
/// EFI memory map: an [`EFI_MAP_HEADER_SIZE`] header, then descriptors.
pub const MODINFOMD_EFI_MAP: u32 = 0x1004;
/// EFI framebuffer description.
pub const MODINFOMD_EFI_FB: u32 = 0x1005;
/// `modulep` as seen by the loader.
pub const MODINFOMD_MODULEP: u32 = 0x1006;
/// VBE framebuffer description.
pub const MODINFOMD_VBE_FB: u32 = 0x1007;
/// EFI architecture of the loader.
pub const MODINFOMD_EFI_ARCH: u32 = 0x1008;

/// `struct bios_smap { u64 base; u64 length; u32 type; } __packed`.
pub const SMAP_ENTRY_SIZE: usize = 20;
/// Offsets inside a `bios_smap` entry.
pub const SMAP_BASE: usize = 0;
/// See [`SMAP_BASE`].
pub const SMAP_LENGTH: usize = 8;
/// See [`SMAP_BASE`].
pub const SMAP_TYPE: usize = 16;
/// `SMAP_TYPE_MEMORY`: usable RAM.
pub const SMAP_TYPE_MEMORY: u32 = 1;

/// `struct efi_map_header { u64 memory_size; u64 descriptor_size;
/// u32 descriptor_version; }`, padded to 16 bytes before the descriptors.
pub const EFI_MAP_HEADER_SIZE: usize = 32;
/// Offsets inside `efi_map_header`.
pub const EFI_MAP_MEMORY_SIZE: usize = 0;
/// See [`EFI_MAP_MEMORY_SIZE`].
pub const EFI_MAP_DESCRIPTOR_SIZE: usize = 8;

/// Offsets inside an EFI memory descriptor (`struct efi_md`).
pub const EFI_MD_TYPE: usize = 0;
/// See [`EFI_MD_TYPE`].
pub const EFI_MD_PHYS: usize = 8;
/// See [`EFI_MD_TYPE`].
pub const EFI_MD_PAGES: usize = 24;
/// EFI pages are always 4 KiB, whatever the kernel page size.
pub const EFI_PAGE_SIZE: u64 = 4096;

/// EFI memory types the kernel may use once boot services are gone
/// (the same set FreeBSD's `add_efi_map_entries` accepts).
pub const EFI_MD_TYPE_USABLE: [u32; 5] = [
    1, // EfiLoaderCode
    2, // EfiLoaderData
    3, // EfiBootServicesCode
    4, // EfiBootServicesData
    7, // EfiConventionalMemory
];
