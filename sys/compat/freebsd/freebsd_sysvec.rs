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

//! The FreeBSD `sysentvec` (`elf64_freebsd_sysvec`): what makes a
//! binary FreeBSD's, as `imgact_elf.c` decides it.

use kern::kern_abi::{ExecImage, SysEntVec};
use sys::elf::notes;

use crate::arch::{ELF_ARCH, SV_NAME};

/// `ELFOSABI_FREEBSD`.
const ELFOSABI_FREEBSD: u8 = 9;
/// `NT_FREEBSD_ABI_TAG`, in a note owned by `FreeBSD`.
const NT_FREEBSD_ABI_TAG: u32 = 1;

/// The FreeBSD ABI.
pub struct FreebsdSysvec;

/// The instance the kernel binary registers.
pub static ELF64_FREEBSD_SYSVEC: FreebsdSysvec = FreebsdSysvec;

impl SysEntVec for FreebsdSysvec {
    fn name(&self) -> &'static str {
        SV_NAME
    }

    /// A FreeBSD binary for this machine: `EI_OSABI` is FreeBSD, or it
    /// carries a `FreeBSD` ABI tag note (`brandinfo`).
    fn accepts(&self, image: &ExecImage<'_>) -> bool {
        let Some(e) = image.ehdr else { return false };
        e.machine == ELF_ARCH
            && (e.osabi == ELFOSABI_FREEBSD
                || notes(image.header, &e)
                    .any(|n| n.name == b"FreeBSD" && n.kind == NT_FREEBSD_ABI_TAG))
    }
}
