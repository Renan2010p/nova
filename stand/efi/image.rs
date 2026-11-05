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


//! Starting another EFI program (`LoadImage`, `StartImage`): another
//! loader, an operating system's boot manager, a Linux kernel through its
//! EFI stub; and the initrd a Linux kernel asks for
//! (`LINUX_EFI_INITRD_MEDIA_GUID`, through `EFI_LOAD_FILE2_PROTOCOL`).

#![allow(unsafe_code)]

use core::ffi::c_void;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use libkern::conv::{u64_from_usize, usize_from_u64};

use crate::Error;
use crate::file::{LOADED_IMAGE_GUID, LoadedImage, loaded_image, protocol};
use crate::mem::Pages;
use crate::table::{Guid, Handle, Status, boot_services, image};

const DEVICE_PATH_GUID: Guid = Guid(0x0957_6e91, 0x6d3f, 0x11d2, [0x8e, 0x39, 0x00, 0xa0, 0xc9, 0x69, 0x72, 0x3b]);
const LOAD_FILE2_GUID: Guid = Guid(0x4006_c0c1, 0xfcb3, 0x403e, [0x99, 0x6d, 0x4a, 0x6c, 0x87, 0x24, 0xe0, 0x6d]);
const EFI_NATIVE_INTERFACE: u32 = 0;

// Device path nodes: type, subtype, length (2 bytes, little-endian).
const MEDIA: u8 = 4;
const MEDIA_VENDOR: u8 = 3;
const MEDIA_FILE_PATH: u8 = 4;
const END: [u8; 4] = [0x7f, 0xff, 4, 0];

/// The longest device path built here, in bytes.
const PATH_MAX: usize = 1024;

/// Appends `b` at `*n` in `out`.
fn push(out: &mut [u8], n: &mut usize, b: &[u8]) -> Result<(), Error> {
    out.get_mut(*n..*n + b.len()).ok_or(Error::TooLarge)?.copy_from_slice(b);
    *n += b.len();
    Ok(())
}

/// The device path of `path` on the volume the stand came from: the
/// volume's own path, then a file path node. The program started can then
/// find its volume, as if the firmware had started it.
fn file_device_path(path: &str, out: &mut [u8; PATH_MAX]) -> Result<(), Error> {
    let li = loaded_image()?;
    let dev: &mut u8 = protocol(li.device_handle, &DEVICE_PATH_GUID)?;
    let base: *const u8 = dev;
    let mut n = 0;
    loop {
        // SAFETY: the firmware's device path: nodes of at least 4 bytes,
        // each giving its length, ended by an end node. `n` is the offset
        // of the next node, which the loop never passes.
        let head = unsafe { core::slice::from_raw_parts(base.add(n), 4) };
        let len = usize::from(u16::from_le_bytes([head[2], head[3]]));
        if head[0] == 0x7f && head[1] == 0xff {
            break;
        }
        if len < 4 || n + len > PATH_MAX / 2 {
            return Err(Error::TooLarge);
        }
        // SAFETY: as above; the node is `len` bytes.
        let node = unsafe { core::slice::from_raw_parts(base.add(n), len) };
        push(out, &mut n, node)?;
    }
    // The file path node: UTF-16, backslashes, NUL-terminated.
    let units = path.encode_utf16().count() + 1;
    let len = u16::try_from(4 + 2 * units).map_err(|_| Error::TooLarge)?;
    let [l0, l1] = len.to_le_bytes();
    push(out, &mut n, &[MEDIA, MEDIA_FILE_PATH, l0, l1])?;
    for u in path.encode_utf16().chain([0]) {
        let u = if u == u16::from(b'/') { u16::from(b'\\') } else { u };
        push(out, &mut n, &u.to_le_bytes())?;
    }
    push(out, &mut n, &END)
}

/// Starts the EFI program `data`, read from `path` on the boot volume,
/// with `options` as its command line (`LoadOptions`, UTF-16). Returns
/// the program's exit status if it returns.
pub fn start_image(path: &str, data: &[u8], options: &str) -> Result<Status, Error> {
    let bs = boot_services()?;
    let mut dp = [0u8; PATH_MAX];
    file_device_path(path, &mut dp)?;
    let mut child: Handle = core::ptr::null_mut();
    // SAFETY: a boot service; `dp` is a device path ended by an end node,
    // `data` is `data.len()` readable bytes, `child` a local out pointer.
    unsafe {
        (bs.load_image)(0, image(), dp.as_ptr().cast(), data.as_ptr().cast(), data.len(), &mut child)
    }
    .ok()?;
    let mut opts = [0u16; 1024];
    let mut n = 0;
    for u in options.encode_utf16() {
        *opts.get_mut(n).ok_or(Error::TooLarge)? = u;
        n += 1;
    }
    if n > 0 {
        let li: &mut LoadedImage = protocol(child, &LOADED_IMAGE_GUID)?;
        // `opts` outlives the program's run below; NUL-terminated.
        li.load_options = opts.as_mut_ptr().cast();
        li.load_options_size = u32::try_from(2 * (n + 1)).map_err(|_| Error::TooLarge)?;
    }
    let mut exit_size = 0;
    let mut exit_data: *mut u16 = core::ptr::null_mut();
    // SAFETY: a boot service, on the image just loaded; out pointers to
    // locals.
    Ok(unsafe { (bs.start_image)(child, &mut exit_size, &mut exit_data) })
}

/// The initrd lent to Linux: where it is and how long.
static INITRD_ADDR: AtomicU64 = AtomicU64::new(0);
static INITRD_LEN: AtomicU64 = AtomicU64::new(0);
static INITRD_INSTALLED: AtomicBool = AtomicBool::new(false);

/// `EFI_LOAD_FILE2_PROTOCOL`.
#[repr(C)]
struct LoadFile2 {
    load_file: unsafe extern "efiapi" fn(*mut LoadFile2, *const c_void, u8, *mut usize, *mut c_void) -> Status,
}

static INITRD_LOAD_FILE2: LoadFile2 = LoadFile2 { load_file: initrd_load_file };

/// A vendor media node with `LINUX_EFI_INITRD_MEDIA_GUID`, then the end:
/// where the Linux EFI stub looks for its initrd.
static INITRD_DEVICE_PATH: [u8; 24] = [
    MEDIA, MEDIA_VENDOR, 20, 0, // node header
    0x27, 0xe4, 0x68, 0x55, 0xfc, 0x68, 0x3d, 0x4f, 0xac, 0x74, 0xca, 0x55, 0x52, 0x31, 0xcc, 0x68, // GUID
    0x7f, 0xff, 4, 0, // end
];

/// `LoadFile`: copies the initrd into the caller's buffer, or says how
/// large it must be.
unsafe extern "efiapi" fn initrd_load_file(
    _this: *mut LoadFile2,
    _path: *const c_void,
    boot_policy: u8,
    size: *mut usize,
    buf: *mut c_void,
) -> Status {
    if boot_policy != 0 {
        return Status::UNSUPPORTED;
    }
    // SAFETY: the caller's size, if not null (UEFI 2.x, 13.2).
    let Some(size) = (unsafe { size.as_mut() }) else { return Status::INVALID_PARAMETER };
    let len = usize_from_u64(INITRD_LEN.load(Ordering::Acquire));
    if buf.is_null() || *size < len {
        *size = len;
        return Status::BUFFER_TOO_SMALL;
    }
    let src = core::ptr::with_exposed_provenance::<u8>(usize_from_u64(INITRD_ADDR.load(Ordering::Acquire)));
    // SAFETY: the initrd's pages, kept (never freed) by `install_initrd`,
    // `len` bytes; the caller's buffer has at least `len` bytes.
    unsafe { core::ptr::copy_nonoverlapping(src, buf.cast::<u8>(), len) };
    *size = len;
    Status::SUCCESS
}

/// Lends the first `len` bytes of `pages` as the initrd of the Linux
/// kernel started next. The pages are kept until the kernel takes them.
pub fn install_initrd(pages: Pages, len: usize) -> Result<(), Error> {
    if len > pages.len() {
        return Err(Error::TooLarge);
    }
    INITRD_ADDR.store(pages.addr(), Ordering::Release);
    INITRD_LEN.store(u64_from_usize(len), Ordering::Release);
    // Installed once: a second boot attempt only changes what it serves.
    if INITRD_INSTALLED.swap(true, Ordering::AcqRel) {
        return Ok(());
    }
    let bs = boot_services()?;
    let mut handle: Handle = core::ptr::null_mut();
    // SAFETY: boot services; both interfaces are statics the firmware only
    // reads, valid for the whole run; `handle` is a local out pointer (null
    // asks for a new handle, and the second call adds to it).
    unsafe {
        (bs.install_protocol_interface)(
            &mut handle,
            &DEVICE_PATH_GUID,
            EFI_NATIVE_INTERFACE,
            core::ptr::addr_of!(INITRD_DEVICE_PATH).cast_mut().cast(),
        )
        .ok()?;
        (bs.install_protocol_interface)(
            &mut handle,
            &LOAD_FILE2_GUID,
            EFI_NATIVE_INTERFACE,
            core::ptr::addr_of!(INITRD_LOAD_FILE2).cast_mut().cast(),
        )
        .ok()
    }
}
