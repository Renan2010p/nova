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


//! `linux`: booting Linux through its EFI stub (`CONFIG_EFI_STUB`, every
//! distribution's kernel): the kernel is an EFI program; it is started
//! with its command line, and finds its initrd where the stub looks for
//! one (`LINUX_EFI_INITRD_MEDIA_GUID`), as systemd-boot and Limine lend
//! it. The stub does the rest: boot parameters, the memory map, leaving
//! the firmware.
//!
//! `stand.conf` options of a `protocol: linux` entry: `path` (the kernel,
//! `kernel_path` too), `cmdline` (`kernel_cmdline` too), `module_path`
//! (initrds, repeated as needed: they are concatenated, as Linux allows).
//!
//! TODO(linux): kernels without the EFI stub (the bzImage boot protocol
//! itself), and the stand on BIOS.

#![no_std]
#![forbid(unsafe_code)]

use common::config::{Entry, boot_path};
use efi::{Error, Pages, Status};
use libkern::conv::{u64_from_usize, usize_from_u64};

/// The most initrd files an entry may name.
const MAX_MODULES: usize = 8;

/// Why booting stopped.
#[derive(Debug)]
pub enum Fail {
    /// The entry has no `path` on the boot volume.
    NoPath,
    /// A `module_path` is not on the boot volume, or there are too many.
    BadModule,
    /// The firmware failed (reading, starting).
    Efi(Error),
    /// The kernel's EFI stub returned this status: it could not boot.
    Returned(Status),
}

impl From<Error> for Fail {
    fn from(e: Error) -> Self {
        Fail::Efi(e)
    }
}

/// Reads the entry's initrds and lends them, concatenated, to the kernel.
fn initrd(entry: &Entry<'_>) -> Result<(), Fail> {
    let mut files: [Option<(Pages, usize)>; MAX_MODULES] = [const { None }; MAX_MODULES];
    let mut total = 0;
    for (i, m) in entry.get_all("module_path").enumerate() {
        let path = boot_path(m).ok_or(Fail::BadModule)?;
        let slot = files.get_mut(i).ok_or(Fail::BadModule)?;
        let (pages, len) = efi::read_file(path)?;
        total += len;
        *slot = Some((pages, len));
    }
    if total == 0 {
        return Ok(());
    }
    let mut all = Pages::alloc(usize_from_u64(u64_from_usize(total).div_ceil(4096)), None)?;
    let buf = all.bytes_mut();
    let mut at = 0;
    for (pages, len) in files.iter().flatten() {
        let src = pages.bytes().get(..*len).ok_or(Fail::BadModule)?;
        buf.get_mut(at..at + len).ok_or(Fail::BadModule)?.copy_from_slice(src);
        at += len;
    }
    efi::install_initrd(all, total)?;
    Ok(())
}

/// Boots the kernel of a `protocol: linux` entry. Returns only if it could
/// not.
pub fn boot(entry: &Entry<'_>) -> Result<core::convert::Infallible, Fail> {
    let path = entry.get("path").or_else(|| entry.get("kernel_path")).and_then(boot_path).ok_or(Fail::NoPath)?;
    let cmdline = entry.get("cmdline").or_else(|| entry.get("kernel_cmdline")).unwrap_or("");
    let (image, len) = efi::read_file(path)?;
    initrd(entry)?;
    let data = image.bytes().get(..len).ok_or(Fail::Efi(Error::TooLarge))?;
    Err(Fail::Returned(efi::start_image(path, data, cmdline)?))
}
