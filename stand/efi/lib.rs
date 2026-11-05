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

//! `efi`: the UEFI interface of the Nova stand (FreeBSD's
//! `stand/efi/libefi`), written against the UEFI 2.x specification, with
//! no crate from elsewhere.
//!
//! The trusted part of the stand, as the framework is the kernel's: the
//! firmware's tables are raw pointers, and this crate turns them into a
//! safe interface. It keeps the system table given to the entry point and
//! knows when the boot services end ([`exit_boot_services`]): after that,
//! every call that needs them fails instead of jumping into freed memory.
//!
//! What the loader needs and no more: the console, pages of memory, the
//! files of the volume the loader came from, the memory map, the graphics
//! output, the configuration tables (ACPI), starting another EFI program
//! (and lending it an initrd, Linux's way), and leaving the boot services.

#![no_std]
#![deny(unsafe_code)]

mod console;
mod file;
mod gop;
mod image;
mod input;
mod mem;
mod table;

pub use console::Console;
pub use file::read_file;
pub use gop::{Framebuffer, Surface, framebuffer};
pub use image::{install_initrd, start_image};
pub use input::{Key, read_key};
pub use mem::{MemoryMap, Pages, exit_boot_services, memory_map};
pub use table::{
    ACPI_20_TABLE_GUID, ACPI_TABLE_GUID, Guid, Handle, Status, SystemTable, config_table, init,
    firmware_setup_supported, image_base, reboot_to_firmware, reset_system, stall, system_table_addr,
};

/// Why a firmware call failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The firmware returned this status.
    Status(Status),
    /// The boot services were already exited.
    NoBootServices,
    /// Something the loader needs is not there (a protocol, a table).
    NotFound,
    /// A size or count does not fit.
    TooLarge,
}
