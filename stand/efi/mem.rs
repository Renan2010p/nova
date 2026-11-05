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

//! Memory from the firmware (`AllocatePages`), the memory map, and the end
//! of the boot services.

#![allow(unsafe_code)]

use core::sync::atomic::Ordering;

use libkern::conv::usize_from_u64;

use crate::table::{EXITED, Status, boot_services, image};
use crate::Error;

/// `AllocateAnyPages`, `AllocateMaxAddress`.
const ALLOCATE_ANY_PAGES: u32 = 0;
const ALLOCATE_MAX_ADDRESS: u32 = 1;
/// `EfiLoaderData`: memory the loader owns, which the kernel sees as such.
const EFI_LOADER_DATA: u32 = 2;
/// UEFI pages are 4 KiB on every architecture.
pub const EFI_PAGE_SIZE: u64 = 4096;

/// Pages the firmware gave the loader. They are not given back: the kernel
/// is loaded into them, and they outlive the loader (`EfiLoaderData`).
pub struct Pages {
    addr: u64,
    count: usize,
}

impl Pages {
    /// `count` pages anywhere (below `max`, if given), zeroed.
    pub fn alloc(count: usize, max: Option<u64>) -> Result<Pages, Error> {
        let bs = boot_services()?;
        let (kind, mut addr) = match max {
            Some(m) => (ALLOCATE_MAX_ADDRESS, m),
            None => (ALLOCATE_ANY_PAGES, 0),
        };
        // SAFETY: a boot service, with an out pointer to a local.
        unsafe { (bs.allocate_pages)(kind, EFI_LOADER_DATA, count, &mut addr) }.ok()?;
        let mut pages = Pages { addr, count };
        pages.bytes_mut().fill(0);
        Ok(pages)
    }

    /// Physical address (UEFI identity-maps memory, so also the address).
    pub fn addr(&self) -> u64 {
        self.addr
    }

    /// Size in bytes.
    pub fn len(&self) -> usize {
        self.count * usize_from_u64(EFI_PAGE_SIZE)
    }

    /// True for no pages.
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// The pages' contents.
    pub fn bytes(&self) -> &[u8] {
        // SAFETY: the firmware gave these pages to the loader alone
        // (`AllocatePages`), UEFI identity-maps memory, and they are never
        // freed; `&self` allows only shared views.
        unsafe { core::slice::from_raw_parts(core::ptr::with_exposed_provenance::<u8>(usize_from_u64(self.addr)), self.len()) }
    }

    /// The pages' contents, to write.
    pub fn bytes_mut(&mut self) -> &mut [u8] {
        // SAFETY: as in `bytes`; `&mut self` makes this view exclusive.
        unsafe {
            core::slice::from_raw_parts_mut(core::ptr::with_exposed_provenance_mut::<u8>(usize_from_u64(self.addr)), self.len())
        }
    }
}

/// A copy of the memory map (`GetMemoryMap`): the descriptors as the
/// firmware lays them out, for the kernel (`MODINFOMD_EFI_MAP`).
pub struct MemoryMap<'a> {
    /// The descriptors, back to back.
    pub descriptors: &'a [u8],
    /// Bytes per descriptor (may grow with UEFI versions).
    pub descriptor_size: usize,
    /// `EFI_MEMORY_DESCRIPTOR_VERSION`.
    pub descriptor_version: u32,
    key: usize,
}

/// Reads the memory map into `buf`. `TooLarge` if it does not fit.
pub fn memory_map(buf: &mut [u8]) -> Result<MemoryMap<'_>, Error> {
    let bs = boot_services()?;
    let (mut size, mut key, mut dsize, mut dver) = (buf.len(), 0usize, 0usize, 0u32);
    // SAFETY: a boot service; `buf` has `size` writable bytes.
    let st = unsafe { (bs.get_memory_map)(&mut size, buf.as_mut_ptr(), &mut key, &mut dsize, &mut dver) };
    if st == Status::BUFFER_TOO_SMALL {
        return Err(Error::TooLarge);
    }
    st.ok()?;
    Ok(MemoryMap { descriptors: buf.get(..size).ok_or(Error::TooLarge)?, descriptor_size: dsize, descriptor_version: dver, key })
}

/// Leaves the boot services with the memory map just read, whose key the
/// firmware checks: nothing may allocate between reading it and this.
/// After this, the firmware's services and console are gone.
pub fn exit_boot_services(map: &MemoryMap<'_>) -> Result<(), Error> {
    let bs = boot_services()?;
    // SAFETY: a boot service, with the loader's image handle and the key
    // of the latest memory map.
    unsafe { (bs.exit_boot_services)(image(), map.key) }.ok()?;
    EXITED.store(true, Ordering::Release);
    Ok(())
}
