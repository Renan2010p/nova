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

//! Loading an ELF kernel (FreeBSD's `stand/common/load_elf.c`): its
//! `PT_LOAD` segments are copied to the physical addresses they are linked
//! for, minus the machine's offset (on amd64, `KERNBASE`).

use libkern::conv::usize_from_u64;
use sys::elf::{Ehdr, PT_LOAD, phdrs};

/// What the loaded kernel looks like.
#[derive(Clone, Copy, Debug)]
pub struct Kernel {
    /// Virtual entry point.
    pub entry: u64,
    /// Lowest physical address of a segment (where the kernel loads).
    pub phys_start: u64,
    /// End of the last segment, `bss` included.
    pub phys_end: u64,
    /// The ELF header, for `MODINFOMD_ELFHDR`.
    pub ehdr: Ehdr,
}

/// Why a kernel could not be loaded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoadError {
    /// Not a 64-bit ELF executable.
    NotElf,
    /// A segment lies outside the file or the staging area.
    BadSegment,
    /// The staging area is too small.
    TooBig,
}

/// The kernel's layout: `virt_to_phys` turns a segment's virtual address
/// into the physical one it must be loaded at.
pub fn layout(file: &[u8], virt_to_phys: impl Fn(u64) -> u64) -> Result<Kernel, LoadError> {
    let ehdr = Ehdr::parse(file).ok_or(LoadError::NotElf)?;
    let mut start = u64::MAX;
    let mut end = 0;
    for ph in phdrs(file, &ehdr).filter(|p| p.kind == PT_LOAD) {
        let pa = virt_to_phys(ph.vaddr);
        start = start.min(pa);
        end = end.max(pa.checked_add(ph.memsz).ok_or(LoadError::BadSegment)?);
    }
    if start == u64::MAX {
        return Err(LoadError::NotElf);
    }
    Ok(Kernel { entry: ehdr.entry, phys_start: start, phys_end: end, ehdr })
}

/// Copies the kernel's segments into `staging`, which will be placed at
/// physical `kernel.phys_start`; `bss` is left zero (the staging area is).
pub fn load(file: &[u8], kernel: &Kernel, virt_to_phys: impl Fn(u64) -> u64, staging: &mut [u8]) -> Result<(), LoadError> {
    for ph in phdrs(file, &kernel.ehdr).filter(|p| p.kind == PT_LOAD) {
        let off = usize_from_u64(virt_to_phys(ph.vaddr) - kernel.phys_start);
        let src = file
            .get(usize_from_u64(ph.offset)..)
            .and_then(|s| s.get(..usize_from_u64(ph.filesz)))
            .ok_or(LoadError::BadSegment)?;
        let dst = staging.get_mut(off..off + src.len()).ok_or(LoadError::TooBig)?;
        dst.copy_from_slice(src);
    }
    Ok(())
}
