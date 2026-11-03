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

//! Machine-dependent bootstrap (FreeBSD's `amd64/machdep.c`).
//!
//! `hammer_time` turns what the loader left behind into an MI
//! [`BootInfo`] and hands it to `mi_startup`. Today it sets up the boot
//! CPU's per-CPU area and the early console; GDT, IDT, TSS and the initial
//! page tables will be added here as the kernel grows. The application
//! processors will start in `mp_machdep.rs` (`init_secondary`).

use mdi::{BootInfo, Firmware, PhysRange};
use sys::bytes::{read_u32, read_u64};
use sys::linker::{MODINFOMD_ENVP, MODINFOMD_HOWTO, Module, Preload};
use sys::physmem::PhysMap;

use crate::EARLY_CONS;
use crate::metadata::{
    EFI_MAP_DESCRIPTOR_SIZE, EFI_MAP_HEADER_SIZE, EFI_MAP_MEMORY_SIZE, EFI_MD_PAGES, EFI_MD_PHYS,
    EFI_MD_TYPE, EFI_MD_TYPE_USABLE, EFI_PAGE_SIZE, MODINFOMD_EFI_MAP, MODINFOMD_SMAP, SMAP_BASE,
    SMAP_ENTRY_SIZE, SMAP_LENGTH, SMAP_TYPE, SMAP_TYPE_MEMORY,
};
use crate::pcpu::pcpu_init;
use crate::vmparam::{BOOT_MAPPED, KERNBASE, KERNLOAD};

/// Maximum number of distinct usable RAM ranges we keep.
const PHYSMAP_SIZE: usize = 128;

unsafe extern "Rust" {
    /// The MI entry point (`mdi::MiStartup`), defined in the kernel binary
    /// by `framework::kernel_entry!`. This is the upcall that lets MD code
    /// start the MI kernel without depending on it.
    fn __nova_mi_startup(boot: &BootInfo<'_>) -> !;
}

/// First Rust code to run, called by `btext` on the boot stack with
/// interrupts disabled.
pub(crate) extern "C" fn hammer_time(modulep: u64, kernend: u64) -> ! {
    // First, so that `curcpu()` works for everything that follows.
    pcpu_init(0);
    EARLY_CONS.init();

    let preload = Preload::new(boot_data(modulep, kernend));
    let kmdp = preload.kernel();

    let howto = kmdp
        .and_then(|k| k.md(MODINFOMD_HOWTO))
        .and_then(|r| r.as_u32())
        .unwrap_or(0);

    // The BIOS and EFI loaders put the environment after the metadata,
    // userboot (bhyveload) puts it before; either way it lies between the
    // loaded files and `kernend`, and ends with an empty string.
    let envp = kmdp
        .and_then(|k| k.md(MODINFOMD_ENVP))
        .and_then(|r| r.as_u64())
        .map_or(&[][..], |envp| boot_data(envp, kernend));

    let mut physmap = PhysMap::<PHYSMAP_SIZE>::new();
    let firmware = kmdp.map_or(Firmware::Unknown, |k| getmemsize(k, &mut physmap));

    let boot = BootInfo {
        firmware,
        physmem: physmap.as_slice(),
        kernel: PhysRange {
            start: KERNLOAD,
            end: kernend,
        },
        howto,
        envp,
        preload,
    };

    // SAFETY: `__nova_mi_startup` is defined exactly once, by
    // `framework::kernel_entry!`, with this same signature (it coerces the
    // MI function to `mdi::MiStartup`). We are on the boot CPU with
    // interrupts disabled and the early console ready, as it requires.
    unsafe { __nova_mi_startup(&boot) }
}

/// Physical `[start, kernend)`, the tail of what the loader placed after
/// the kernel (metadata, environment), as a byte slice. Empty if the range
/// is not inside the loader's data.
fn boot_data(start: u64, kernend: u64) -> &'static [u8] {
    if start < KERNLOAD || kernend <= start || kernend > BOOT_MAPPED {
        return &[];
    }
    // SAFETY: the loader stored the kernel, modules, metadata and
    // environment in [KERNLOAD, kernend), below BOOT_MAPPED, which its page
    // tables map at KERNBASE (both checked above). That memory is reported
    // to MI code as `BootInfo::kernel`, so nothing will ever reuse it: the
    // slice is valid and immutable for the life of the kernel.
    unsafe {
        core::slice::from_raw_parts((KERNBASE + start) as *const u8, (kernend - start) as usize)
    }
}

/// Collects usable RAM from the firmware memory map the loader passed
/// (`getmemsize`). Prefers the EFI map, as FreeBSD does.
fn getmemsize<const N: usize>(kmdp: Module<'_>, physmap: &mut PhysMap<N>) -> Firmware {
    if let Some(r) = kmdp.md(MODINFOMD_EFI_MAP) {
        add_efi_map_entries(r.data, physmap);
        Firmware::Uefi
    } else if let Some(r) = kmdp.md(MODINFOMD_SMAP) {
        add_smap_entries(r.data, physmap);
        Firmware::Bios
    } else {
        Firmware::Unknown
    }
}

/// BIOS E820 map: an array of packed `bios_smap` entries.
fn add_smap_entries<const N: usize>(smap: &[u8], physmap: &mut PhysMap<N>) {
    for e in smap.as_chunks::<SMAP_ENTRY_SIZE>().0 {
        let (Some(base), Some(len), Some(ty)) = (
            read_u64(e, SMAP_BASE),
            read_u64(e, SMAP_LENGTH),
            read_u32(e, SMAP_TYPE),
        ) else {
            continue;
        };
        if ty == SMAP_TYPE_MEMORY {
            physmap.add(base, len);
        }
    }
}

/// EFI memory map: `efi_map_header`, then `memory_size` bytes of
/// descriptors, each `descriptor_size` bytes long.
fn add_efi_map_entries<const N: usize>(map: &[u8], physmap: &mut PhysMap<N>) -> Option<()> {
    let memory_size = usize::try_from(read_u64(map, EFI_MAP_MEMORY_SIZE)?).ok()?;
    let descriptor_size = usize::try_from(read_u64(map, EFI_MAP_DESCRIPTOR_SIZE)?).ok()?;
    if descriptor_size == 0 {
        return None;
    }
    let descs = map.get(EFI_MAP_HEADER_SIZE..)?;
    let descs = descs.get(..memory_size.min(descs.len()))?;
    for d in descs.chunks_exact(descriptor_size) {
        let ty = read_u32(d, EFI_MD_TYPE)?;
        if EFI_MD_TYPE_USABLE.contains(&ty) {
            let pages = read_u64(d, EFI_MD_PAGES)?;
            physmap.add(
                read_u64(d, EFI_MD_PHYS)?,
                pages.saturating_mul(EFI_PAGE_SIZE),
            );
        }
    }
    Some(())
}
