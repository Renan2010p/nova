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

//! The UEFI system table and boot services (UEFI 2.x, chapter 4), and the
//! state the rest of the crate builds on.

#![allow(unsafe_code)]

use core::ffi::c_void;
use core::sync::atomic::{AtomicBool, AtomicPtr, Ordering};

use crate::Error;

/// A handle (`EFI_HANDLE`).
pub type Handle = *mut c_void;

/// A status code (`EFI_STATUS`): 0 is success, the high bit marks errors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(transparent)]
pub struct Status(pub usize);

impl Status {
    /// `EFI_SUCCESS`.
    pub const SUCCESS: Status = Status(0);
    /// `EFI_LOAD_ERROR`.
    pub const LOAD_ERROR: Status = Status(1 | ERROR_BIT);
    /// `EFI_INVALID_PARAMETER`.
    pub const INVALID_PARAMETER: Status = Status(2 | ERROR_BIT);
    /// `EFI_UNSUPPORTED`.
    pub const UNSUPPORTED: Status = Status(3 | ERROR_BIT);
    /// `EFI_BUFFER_TOO_SMALL`.
    pub const BUFFER_TOO_SMALL: Status = Status(5 | ERROR_BIT);

    /// `Ok(())` for success, the status otherwise.
    pub fn ok(self) -> Result<(), Error> {
        if self == Status::SUCCESS { Ok(()) } else { Err(Error::Status(self)) }
    }
}

const ERROR_BIT: usize = 1 << (usize::BITS - 1);

/// A GUID (`EFI_GUID`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct Guid(pub u32, pub u16, pub u16, pub [u8; 8]);

/// `EFI_ACPI_20_TABLE_GUID`.
pub const ACPI_20_TABLE_GUID: Guid =
    Guid(0x8868_e871, 0xe4f1, 0x11d3, [0xbc, 0x22, 0x00, 0x80, 0xc7, 0x3c, 0x88, 0x81]);
/// `ACPI_TABLE_GUID` (ACPI 1.0).
pub const ACPI_TABLE_GUID: Guid =
    Guid(0xeb9d_2d30, 0x2d88, 0x11d3, [0x9a, 0x16, 0x00, 0x90, 0x27, 0x3f, 0xc1, 0x4d]);

#[repr(C)]
struct TableHeader {
    signature: u64,
    revision: u32,
    header_size: u32,
    crc32: u32,
    reserved: u32,
}

/// `EFI_SYSTEM_TABLE`.
#[repr(C)]
pub struct SystemTable {
    hdr: TableHeader,
    firmware_vendor: *const u16,
    firmware_revision: u32,
    console_in_handle: Handle,
    pub(crate) con_in: *mut crate::input::SimpleTextInput,
    console_out_handle: Handle,
    pub(crate) con_out: *mut crate::console::SimpleTextOutput,
    standard_error_handle: Handle,
    std_err: *mut c_void,
    pub(crate) runtime_services: *mut RuntimeServices,
    pub(crate) boot_services: *mut BootServices,
    number_of_table_entries: usize,
    configuration_table: *const ConfigurationTable,
}

#[repr(C)]
struct ConfigurationTable {
    vendor_guid: Guid,
    vendor_table: *mut c_void,
}

type Unused = usize;

/// `EFI_RUNTIME_SERVICES`, up to `ResetSystem`.
#[repr(C)]
pub(crate) struct RuntimeServices {
    hdr: TableHeader,
    get_time: Unused,
    set_time: Unused,
    get_wakeup_time: Unused,
    set_wakeup_time: Unused,
    set_virtual_address_map: Unused,
    convert_pointer: Unused,
    pub(crate) get_variable: unsafe extern "efiapi" fn(*const u16, *const Guid, *mut u32, *mut usize, *mut u8) -> Status,
    get_next_variable_name: Unused,
    pub(crate) set_variable: unsafe extern "efiapi" fn(*const u16, *const Guid, u32, usize, *const u8) -> Status,
    get_next_high_monotonic_count: Unused,
    pub(crate) reset_system: unsafe extern "efiapi" fn(u32, Status, usize, *const c_void) -> !,
}

/// `EFI_BOOT_SERVICES`, in the specification's order. Unused entries are
/// kept as placeholders so that the used ones sit at the right offsets.
#[repr(C)]
pub(crate) struct BootServices {
    hdr: TableHeader,
    raise_tpl: Unused,
    restore_tpl: Unused,
    pub(crate) allocate_pages: unsafe extern "efiapi" fn(u32, u32, usize, *mut u64) -> Status,
    pub(crate) free_pages: unsafe extern "efiapi" fn(u64, usize) -> Status,
    pub(crate) get_memory_map: unsafe extern "efiapi" fn(*mut usize, *mut u8, *mut usize, *mut usize, *mut u32) -> Status,
    allocate_pool: Unused,
    free_pool: Unused,
    create_event: Unused,
    set_timer: Unused,
    wait_for_event: Unused,
    signal_event: Unused,
    close_event: Unused,
    check_event: Unused,
    pub(crate) install_protocol_interface: unsafe extern "efiapi" fn(*mut Handle, *const Guid, u32, *mut c_void) -> Status,
    reinstall_protocol_interface: Unused,
    uninstall_protocol_interface: Unused,
    pub(crate) handle_protocol: unsafe extern "efiapi" fn(Handle, *const Guid, *mut *mut c_void) -> Status,
    reserved: Unused,
    register_protocol_notify: Unused,
    locate_handle: Unused,
    locate_device_path: Unused,
    install_configuration_table: Unused,
    pub(crate) load_image: unsafe extern "efiapi" fn(u8, Handle, *const c_void, *const c_void, usize, *mut Handle) -> Status,
    pub(crate) start_image: unsafe extern "efiapi" fn(Handle, *mut usize, *mut *mut u16) -> Status,
    exit: Unused,
    unload_image: Unused,
    pub(crate) exit_boot_services: unsafe extern "efiapi" fn(Handle, usize) -> Status,
    get_next_monotonic_count: Unused,
    pub(crate) stall: unsafe extern "efiapi" fn(usize) -> Status,
    pub(crate) set_watchdog_timer: unsafe extern "efiapi" fn(usize, u64, usize, *const u16) -> Status,
    connect_controller: Unused,
    disconnect_controller: Unused,
    open_protocol: Unused,
    close_protocol: Unused,
    open_protocol_information: Unused,
    protocols_per_handle: Unused,
    locate_handle_buffer: Unused,
    pub(crate) locate_protocol: unsafe extern "efiapi" fn(*const Guid, *mut c_void, *mut *mut c_void) -> Status,
}

static SYSTEM_TABLE: AtomicPtr<SystemTable> = AtomicPtr::new(core::ptr::null_mut());
static IMAGE: AtomicPtr<c_void> = AtomicPtr::new(core::ptr::null_mut());
/// Set once `ExitBootServices` succeeded: the boot services are gone.
pub(crate) static EXITED: AtomicBool = AtomicBool::new(false);

/// Takes the image handle and system table the firmware passed to the
/// entry point, and turns off the firmware's five-minute watchdog.
///
/// # Safety
///
/// `st` must be the system table the firmware passed to `efi_main`, and
/// `image` its image handle; called once, before anything else here.
pub unsafe fn init(image: Handle, st: *mut SystemTable) {
    SYSTEM_TABLE.store(st, Ordering::Release);
    IMAGE.store(image, Ordering::Release);
    if let Ok(bs) = boot_services() {
        // SAFETY: a boot service of the live table; disabling the watchdog
        // (0 seconds) is what loaders do so that slow boots are not reset.
        let _ = unsafe { (bs.set_watchdog_timer)(0, 0, 0, core::ptr::null()) };
    }
}

pub(crate) fn system_table() -> Result<&'static SystemTable, Error> {
    let st = SYSTEM_TABLE.load(Ordering::Acquire);
    // SAFETY: set by `init` to the firmware's system table, which stays
    // valid (its runtime part) for as long as the loader runs.
    unsafe { st.as_ref() }.ok_or(Error::NotFound)
}

pub(crate) fn image() -> Handle {
    IMAGE.load(Ordering::Acquire)
}

/// The boot services, while they exist.
pub(crate) fn boot_services() -> Result<&'static BootServices, Error> {
    if EXITED.load(Ordering::Acquire) {
        return Err(Error::NoBootServices);
    }
    let st = system_table()?;
    // SAFETY: the firmware's boot services table, valid until
    // `ExitBootServices` (checked above).
    unsafe { st.boot_services.as_ref() }.ok_or(Error::NotFound)
}

/// The address of the system table, for the kernel
/// (`MODINFOMD_FW_HANDLE`).
pub fn system_table_addr() -> u64 {
    libkern::conv::u64_from_usize(SYSTEM_TABLE.load(Ordering::Acquire).addr())
}

/// The configuration table with `guid` (e.g. ACPI's root pointer).
pub fn config_table(guid: &Guid) -> Option<u64> {
    let st = system_table().ok()?;
    for i in 0..st.number_of_table_entries {
        // SAFETY: `i` is below the table's entry count, and the firmware
        // keeps the configuration table alive.
        let t = unsafe { &*st.configuration_table.add(i) };
        if t.vendor_guid == *guid {
            return Some(libkern::conv::u64_from_usize(t.vendor_table.addr()));
        }
    }
    None
}

/// Where the firmware loaded this image, and its size (from
/// `EFI_LOADED_IMAGE_PROTOCOL`).
pub fn image_base() -> Result<(u64, u64), Error> {
    let li = crate::file::loaded_image()?;
    Ok((libkern::conv::u64_from_usize(li.image_base.addr()), li.image_size))
}

/// Waits `us` microseconds (`Stall`).
pub fn stall(us: usize) {
    if let Ok(bs) = boot_services() {
        // SAFETY: a boot service with a plain argument.
        let _ = unsafe { (bs.stall)(us) };
    }
}

/// Restarts the machine (`ResetSystem(EfiResetCold)`).
pub fn reset_system() -> ! {
    if let Ok(st) = system_table()
        // SAFETY: the firmware's runtime services, which outlive the boot
        // services.
        && let Some(rt) = unsafe { st.runtime_services.as_ref() }
    {
        // SAFETY: a runtime service; a cold reset never returns.
        unsafe { (rt.reset_system)(0, Status::SUCCESS, 0, core::ptr::null()) }
    }
    loop {
        core::hint::spin_loop();
    }
}

/// `EFI_GLOBAL_VARIABLE`: the namespace of the specification's variables.
const GLOBAL_VARIABLE: Guid = Guid(0x8be4_df61, 0x93ca, 0x11d2, [0xaa, 0x0d, 0x00, 0xe0, 0x98, 0x03, 0x2b, 0x8c]);
/// `EFI_OS_INDICATIONS_BOOT_TO_FW_UI`.
const BOOT_TO_FW_UI: u64 = 1;
/// `EFI_VARIABLE_NON_VOLATILE | BOOTSERVICE_ACCESS | RUNTIME_ACCESS`.
const VARIABLE_NV_BS_RT: u32 = 0x7;

fn runtime() -> Option<&'static RuntimeServices> {
    let st = system_table().ok()?;
    // SAFETY: the firmware's runtime services table, alive for as long as
    // the machine runs.
    unsafe { st.runtime_services.as_ref() }
}

/// A NUL-terminated UCS-2 copy of an ASCII name.
fn ucs2<const N: usize>(name: &str) -> [u16; N] {
    let mut out = [0u16; N];
    for (o, b) in out.iter_mut().zip(name.bytes()) {
        *o = u16::from(b);
    }
    out
}

/// True if the firmware can be asked to show its setup screen at the next
/// boot (`OsIndicationsSupported` has `BOOT_TO_FW_UI`).
pub fn firmware_setup_supported() -> bool {
    let Some(rt) = runtime() else { return false };
    let name: [u16; 32] = ucs2("OsIndicationsSupported");
    let mut v = [0u8; 8];
    let (mut attrs, mut size) = (0u32, v.len());
    // SAFETY: a runtime service; `name` is NUL-terminated, `v` has `size`
    // writable bytes.
    let st = unsafe { (rt.get_variable)(name.as_ptr(), &GLOBAL_VARIABLE, &mut attrs, &mut size, v.as_mut_ptr()) };
    st == Status::SUCCESS && u64::from_le_bytes(v) & BOOT_TO_FW_UI != 0
}

/// Restarts into the firmware's setup screen (`OsIndications`), as
/// boot loaders' "Firmware Setup" entries do.
pub fn reboot_to_firmware() -> ! {
    if let Some(rt) = runtime() {
        let name: [u16; 16] = ucs2("OsIndications");
        let v = BOOT_TO_FW_UI.to_le_bytes();
        // SAFETY: a runtime service; `name` is NUL-terminated and `v` has
        // the variable's 8 bytes.
        let _ = unsafe { (rt.set_variable)(name.as_ptr(), &GLOBAL_VARIABLE, VARIABLE_NV_BS_RT, v.len(), v.as_ptr()) };
    }
    reset_system()
}
