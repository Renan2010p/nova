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
//! `hammer_time` brings the boot CPU from the loader's state to one the MI
//! kernel can run in, then turns what the loader left behind into an MI
//! [`BootInfo`] and hands it to `mi_startup`:
//!
//! 1. per-CPU area (`%gs`), early console, TSC frequency;
//! 2. the kernel's own GDT, TSS and IDT: from here on, faults are caught
//!    and reported instead of resetting the machine;
//! 3. preload metadata and the firmware memory map;
//! 4. the kernel's own page tables and the direct map (`pmap_bootstrap`);
//! 5. `phys_avail`: the memory the page allocator may use;
//! 6. the local APIC, in x2APIC mode;
//! 7. the CPU list, from the ACPI MADT (`mp_machdep.rs`).
//!
//! The application processors will start in `mp_machdep.rs`
//! (`init_secondary`), repeating steps 1, 2 and 6 for themselves.

use libkern::conv::{u64_from_usize, usize_from_u64};
use mdi::{BootInfo, FbInfo, Firmware, PhysRange};
use sys::bytes::{read_u32, read_u64};
use sys::linker::{
    MODINFOMD_ENVP, MODINFOMD_ESYM, MODINFOMD_HOWTO, MODINFOMD_SSYM, Module, Preload,
};
use sys::physmem::PhysMap;

use crate::EARLY_CONS;
use crate::metadata::{
    EFI_MAP_DESCRIPTOR_SIZE, EFI_MAP_HEADER_SIZE, EFI_MAP_MEMORY_SIZE, EFI_MD_PAGES, EFI_MD_PHYS,
    EFI_MD_TYPE, EFI_MD_TYPE_USABLE, EFI_PAGE_SIZE, MODINFOMD_EFI_FB, MODINFOMD_EFI_MAP, MODINFOMD_SMAP, SMAP_BASE,
    SMAP_ENTRY_SIZE, SMAP_LENGTH, SMAP_TYPE, SMAP_TYPE_MEMORY,
};
use crate::mp_machdep::cpu_mp_setup;
use crate::pcpu::pcpu_init;
use crate::pmap::pmap_bootstrap;
use crate::segments::{boot_ist_tops, cpu_tables_init, idt_init};
use crate::trap::APIC_SPURIOUS_INT;
use crate::vmparam::{BOOT_MAPPED, KERNBASE, KERNLOAD};

/// Low memory kept out of the page allocator: real-mode IVT/BDA, EBDA and
/// the future AP boot trampoline live below 1 MiB.
const ISA_HOLE_END: u64 = 0x10_0000;

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
    x86::tsc::tsc_calibrate();
    idt_init();
    cpu_tables_init(0, boot_ist_tops());

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

    let mut physfree = kernend;
    let dmap_limit = pmap_bootstrap(&mut physfree, physmap.as_slice());

    // phys_avail: RAM minus low memory, the kernel, the loader's data and
    // the early page tables, and minus anything the direct map misses.
    let mut physavail = physmap.clone();
    physavail.remove(0, ISA_HOLE_END.max(physfree));
    physavail.remove(dmap_limit, u64::MAX);

    // The 8259s are not used: mask them before the APIC takes over.
    x86::i8259::atpic_mask_all();
    if let Err(e) = x86::local_apic::lapic_init(APIC_SPURIOUS_INT) {
        panic!("local APIC: {e:?} (TODO(xapic): only x2APIC is supported; under bhyve, use -x)");
    }
    cpu_mp_setup(&sys::kenv::StaticEnv::new(envp), physmap.as_slice());

    // The symbol table, [SSYM, ESYM), for backtraces.
    let md = |t| kmdp.and_then(|k| k.md(t)).and_then(|r| r.as_u64());
    let ksyms = match (md(MODINFOMD_SSYM), md(MODINFOMD_ESYM)) {
        (Some(ssym), Some(esym)) if esym > ssym => {
            let area = boot_data(ssym, kernend);
            &area[..area.len().min(usize_from_u64(esym - ssym))]
        }
        _ => &[],
    };

    // The kernel image itself is live memory, not boot data: start after it.
    let loaded_base = kernel_image_end() - KERNBASE;
    let loaded = boot_data(loaded_base, kernend);

    let framebuffer = md_raw(kmdp, MODINFOMD_EFI_FB).and_then(efi_fb);

    let boot = BootInfo {
        firmware,
        physmem: physmap.as_slice(),
        physavail: physavail.as_slice(),
        kernel: PhysRange {
            start: KERNLOAD,
            end: physfree,
        },
        howto,
        envp,
        loaded,
        loaded_base,
        preload_addr_relocate: KERNBASE,
        framebuffer,
        ksyms,
        preload,
    };

    // SAFETY: `__nova_mi_startup` is defined exactly once, by
    // `framework::kernel_entry!`, with this same signature (it coerces the
    // MI function to `mdi::MiStartup`). We are on the boot CPU with
    // interrupts disabled, the early console ready, traps handled and the
    // direct map in place, as it requires.
    unsafe { __nova_mi_startup(&boot) }
}

/// The payload of the kernel's metadata record `kind`.
fn md_raw(kmdp: Option<Module<'_>>, kind: u32) -> Option<&[u8]> {
    kmdp.and_then(|k| k.md(kind)).map(|r| r.data)
}

/// The UEFI framebuffer from `struct efi_fb` (`x86/include/metadata.h`):
/// address, size, height, width, stride (in pixels), then the red, green,
/// blue and reserved masks.
fn efi_fb(b: &[u8]) -> Option<FbInfo> {
    let fb = FbInfo {
        paddr: read_u64(b, 0)?,
        size: read_u64(b, 8)?,
        height: read_u32(b, 16)?,
        width: read_u32(b, 20)?,
        stride: read_u32(b, 24)?,
        red_mask: read_u32(b, 28)?,
        green_mask: read_u32(b, 32)?,
        blue_mask: read_u32(b, 36)?,
    };
    (fb.paddr != 0 && fb.width > 0 && fb.height > 0 && fb.stride >= fb.width).then_some(fb)
}

/// Virtual address of the end of the kernel image (`end` in the linker
/// script).
fn kernel_image_end() -> u64 {
    unsafe extern "C" {
        static end: u8;
    }
    u64_from_usize(core::ptr::addr_of!(end).addr())
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
        core::slice::from_raw_parts(core::ptr::with_exposed_provenance::<u8>(usize_from_u64(KERNBASE + start)), usize_from_u64(kernend - start))
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
