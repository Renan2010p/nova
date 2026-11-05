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


//! `chainload`: starting another EFI program (Limine's `efi` protocol):
//! Windows' boot manager (`/EFI/Microsoft/Boot/bootmgfw.efi`), another
//! loader (FreeBSD's `/boot/loader.efi`, GRUB, ...), a firmware tool. If
//! the program returns, the menu comes back.
//!
//! `stand.conf` options of a `protocol: efi` (or `efi_chainload`) entry:
//! `path` (`image_path` too), `cmdline` (its `LoadOptions`).

#![no_std]
#![forbid(unsafe_code)]

use common::config::{Entry, boot_path};
use efi::{Error, Status};

/// Why the program could not be started.
#[derive(Debug)]
pub enum Fail {
    /// The entry has no `path` on the boot volume.
    NoPath,
    /// The firmware failed (reading, loading).
    Efi(Error),
}

impl From<Error> for Fail {
    fn from(e: Error) -> Self {
        Fail::Efi(e)
    }
}

/// Starts the program of an `efi` entry; returns its exit status when it
/// returns.
pub fn boot(entry: &Entry<'_>) -> Result<Status, Fail> {
    let path = entry.get("path").or_else(|| entry.get("image_path")).and_then(boot_path).ok_or(Fail::NoPath)?;
    let (image, len) = efi::read_file(path)?;
    let data = image.bytes().get(..len).ok_or(Fail::Efi(Error::TooLarge))?;
    Ok(efi::start_image(path, data, entry.get("cmdline").unwrap_or(""))?)
}
