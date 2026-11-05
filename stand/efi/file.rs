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

//! Files on the volume the loader was loaded from
//! (`EFI_SIMPLE_FILE_SYSTEM_PROTOCOL`): the firmware's own FAT driver, so
//! the loader needs none until it reads UFS itself.

#![allow(unsafe_code)]

use core::ffi::c_void;

use libkern::conv::{u64_from_usize, usize_from_u64};

use crate::mem::{EFI_PAGE_SIZE, Pages};
use crate::table::{Guid, Handle, Status, boot_services, image};
use crate::Error;

pub(crate) const LOADED_IMAGE_GUID: Guid = Guid(0x5b1b_31a1, 0x9562, 0x11d2, [0x8e, 0x3f, 0x00, 0xa0, 0xc9, 0x69, 0x72, 0x3b]);
const SIMPLE_FS_GUID: Guid = Guid(0x964e_5b22, 0x6459, 0x11d2, [0x8e, 0x39, 0x00, 0xa0, 0xc9, 0x69, 0x72, 0x3b]);
const FILE_INFO_GUID: Guid = Guid(0x0957_6e92, 0x6d3f, 0x11d2, [0x8e, 0x39, 0x00, 0xa0, 0xc9, 0x69, 0x72, 0x3b]);
const FILE_MODE_READ: u64 = 1;

/// `EFI_LOADED_IMAGE_PROTOCOL`.
#[repr(C)]
pub(crate) struct LoadedImage {
    revision: u32,
    parent_handle: Handle,
    system_table: *mut c_void,
    pub(crate) device_handle: Handle,
    file_path: *mut c_void,
    reserved: *mut c_void,
    pub(crate) load_options_size: u32,
    pub(crate) load_options: *mut c_void,
    pub(crate) image_base: *mut c_void,
    pub(crate) image_size: u64,
}

#[repr(C)]
struct SimpleFs {
    revision: u64,
    open_volume: unsafe extern "efiapi" fn(*mut SimpleFs, *mut *mut FileProtocol) -> Status,
}

#[repr(C)]
struct FileProtocol {
    revision: u64,
    open: unsafe extern "efiapi" fn(*mut FileProtocol, *mut *mut FileProtocol, *const u16, u64, u64) -> Status,
    close: unsafe extern "efiapi" fn(*mut FileProtocol) -> Status,
    delete: *mut c_void,
    read: unsafe extern "efiapi" fn(*mut FileProtocol, *mut usize, *mut u8) -> Status,
    write: *mut c_void,
    get_position: *mut c_void,
    set_position: *mut c_void,
    get_info: unsafe extern "efiapi" fn(*mut FileProtocol, *const Guid, *mut usize, *mut u8) -> Status,
}

pub(crate) fn protocol<T>(handle: Handle, guid: &Guid) -> Result<&'static mut T, Error> {
    let bs = boot_services()?;
    let mut p: *mut c_void = core::ptr::null_mut();
    // SAFETY: a boot service, with an out pointer to a local.
    unsafe { (bs.handle_protocol)(handle, guid, &mut p) }.ok()?;
    // SAFETY: the firmware returned the interface for `guid`, which has
    // layout `T`; it lives while the boot services do.
    unsafe { p.cast::<T>().as_mut() }.ok_or(Error::NotFound)
}

pub(crate) fn loaded_image() -> Result<&'static mut LoadedImage, Error> {
    protocol(image(), &LOADED_IMAGE_GUID)
}

/// Reads the whole file at `path` (backslashes or slashes) on the boot
/// volume into fresh pages. Returns them and the file's size.
pub fn read_file(path: &str) -> Result<(Pages, usize), Error> {
    let li = loaded_image()?;
    let fs: &mut SimpleFs = protocol(li.device_handle, &SIMPLE_FS_GUID)?;
    let mut root: *mut FileProtocol = core::ptr::null_mut();
    // SAFETY: the volume's protocol; out pointer to a local.
    unsafe { (fs.open_volume)(fs, &mut root) }.ok()?;
    let mut name = [0u16; 256];
    let mut n = 0;
    for c in path.chars() {
        let c = if c == '/' { '\\' } else { c };
        let mut u = [0u16; 2];
        for &x in c.encode_utf16(&mut u).iter() {
            *name.get_mut(n).ok_or(Error::TooLarge)? = x;
            n += 1;
        }
    }
    let mut file: *mut FileProtocol = core::ptr::null_mut();
    // SAFETY: `root` is the open volume; `name` is NUL-terminated (zeroed
    // past `n`, which is below its length).
    let st = unsafe { ((*root).open)(root, &mut file, name.as_ptr(), FILE_MODE_READ, 0) };
    // SAFETY: closing the volume handle we opened.
    unsafe { ((*root).close)(root) };
    st.ok()?;
    let result = read_open(file);
    // SAFETY: closing the file we opened.
    unsafe { ((*file).close)(file) };
    result
}

fn read_open(file: *mut FileProtocol) -> Result<(Pages, usize), Error> {
    // EFI_FILE_INFO: Size, then FileSize (u64) at offset 8.
    let mut info = [0u8; 512];
    let mut len = info.len();
    // SAFETY: `file` is open; `info` has `len` writable bytes.
    unsafe { ((*file).get_info)(file, &FILE_INFO_GUID, &mut len, info.as_mut_ptr()) }.ok()?;
    let size = usize_from_u64(u64::from_le_bytes(info.get(8..16).and_then(|b| b.try_into().ok()).ok_or(Error::TooLarge)?));
    let count = usize_from_u64(u64_from_usize(size).div_ceil(EFI_PAGE_SIZE)).max(1);
    let mut pages = Pages::alloc(count, None)?;
    let buf = pages.bytes_mut();
    let mut done = 0;
    while done < size {
        let mut chunk = (size - done).min(1 << 20);
        let dst = buf.get_mut(done..done + chunk).ok_or(Error::TooLarge)?;
        // SAFETY: `file` is open; `dst` has `chunk` writable bytes.
        unsafe { ((*file).read)(file, &mut chunk, dst.as_mut_ptr()) }.ok()?;
        if chunk == 0 {
            break;
        }
        done += chunk;
    }
    Ok((pages, done))
}
