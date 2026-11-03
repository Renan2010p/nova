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

//! Multiprocessor support (FreeBSD's `amd64/mp_machdep.c`,
//! `x86/x86/mp_x86.c` and `x86/acpica/madt.c`).
//!
//! * `cpu_mp_setup` (from `hammer_time`) finds the CPUs in the ACPI MADT
//!   and numbers them: the boot CPU is 0, the others follow in table order,
//!   as FreeBSD does.
//! * `cpu_start_ap` copies the trampoline (`mpboot.rs`) to low memory,
//!   wakes one AP with INIT-SIPI-SIPI and waits until it reports in.
//! * `init_secondary` is the AP's first kernel code: it repeats for itself
//!   what `hammer_time` did for the boot CPU (page tables, per-CPU area,
//!   descriptor tables, local APIC), then hands over to the framework.

use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};

use mdi::PhysRange;
use sys::acpi::{self, Rsdp};
use sys::kenv::StaticEnv;
use x86::cpufunc::{load_cr0, load_cr3, load_cr4, rcr0, rcr4};
use x86::identcpu::topo_shifts;
use x86::local_apic::{ipi_startup, lapic_id, lapic_init};
use x86::tsc::delay;

use crate::mpboot::{
    MPTRAMP_ADDR, mpt_cpuid, mpt_cr3, mpt_entry, mpt_stack, mptramp_end, mptramp_start,
};
use crate::param::{MAXCPU, PAGE_SIZE};
use crate::pcpu::pcpu_init;
use crate::pmap::{kernel_pml4, mptramp_pml4, phys_to_dmap};
use crate::segments::cpu_tables_init;
use crate::trap::APIC_SPURIOUS_INT;

/// Number of CPUs found (`mp_ncpus` before the APs start).
static MP_NCPUS: AtomicU32 = AtomicU32::new(1);
/// APIC id of each CPU id.
static CPU_APIC_ID: [AtomicU32; MAXCPU] = [const { AtomicU32::new(0) }; MAXCPU];
/// OEM id and OEM table id of the MADT, for the boot message.
static MADT_IDENT: [AtomicU8; 14] = [const { AtomicU8::new(0) }; 14];
static MADT_FOUND: AtomicBool = AtomicBool::new(false);

/// The boot CPU's `%cr0` and `%cr4`, which the APs copy.
static BSP_CR0: AtomicU64 = AtomicU64::new(0);
static BSP_CR4: AtomicU64 = AtomicU64::new(0);

/// IST stack tops for the AP being started (`cpu_start_ap` -> `init_secondary`).
static AP_IST: [AtomicU64; 3] = [const { AtomicU64::new(0) }; 3];
/// Set by the AP being started once it is up (its CPU id).
static AP_STARTED: AtomicU32 = AtomicU32::new(0);

/// How long to wait for an AP to report in (FreeBSD waits 5 s).
const AP_START_TIMEOUT_MS: u32 = 5000;

/// The BIOS areas where an RSDP may be, when the loader does not pass
/// `acpi.rsdp` (bhyve's userboot does not): the start of the EBDA and the
/// BIOS ROM area, as ACPICA's `AcpiFindRootPointer` scans.
const EBDA_PTR: u64 = 0x40e;
const BIOS_ROM: PhysRange = PhysRange {
    start: 0xe_0000,
    end: 0x10_0000,
};

/// A physical-memory reader for `sys::acpi`, through the direct map.
fn phys_read(pa: u64, len: usize) -> Option<&'static [u8]> {
    let end = pa.checked_add(len as u64)?;
    if end > crate::pmap::dmap_limit() {
        return None;
    }
    // SAFETY: the range is inside the direct map (checked), which maps all
    // of it read-write for the life of the kernel. ACPI tables are firmware
    // data nobody writes; the slice is only read.
    Some(unsafe { core::slice::from_raw_parts(phys_to_dmap(pa) as *const u8, len) })
}

fn find_rsdp(kenv: &StaticEnv<'_>) -> Option<Rsdp> {
    if let Some(pa) = kenv.getenv_u64("acpi.rsdp") {
        return phys_read(pa, 36).and_then(acpi::parse_rsdp);
    }
    let ebda = phys_read(EBDA_PTR, 2).map(|b| u64::from(u16::from_le_bytes([b[0], b[1]])) << 4);
    let areas = ebda
        .map(|e| (e, 1024))
        .into_iter()
        .chain([(BIOS_ROM.start, BIOS_ROM.len() as usize)]);
    for (base, len) in areas {
        if let Some(pa) = phys_read(base, len).and_then(|a| acpi::find_rsdp(a, base)) {
            return phys_read(pa, 36).and_then(acpi::parse_rsdp);
        }
    }
    None
}

/// Finds the CPUs (`cpu_mp_probe` / `madt_probe_cpus`). Without a usable
/// MADT, or without RAM for the trampoline, the system runs on the boot
/// CPU alone.
pub(crate) fn cpu_mp_setup(kenv: &StaticEnv<'_>, physmem: &[PhysRange]) {
    let bsp = lapic_id();
    CPU_APIC_ID[0].store(bsp, Ordering::Relaxed);
    BSP_CR0.store(rcr0(), Ordering::Relaxed);
    BSP_CR4.store(rcr4(), Ordering::Relaxed);

    let tramp_ok = physmem
        .iter()
        .any(|r| r.start <= MPTRAMP_ADDR && MPTRAMP_ADDR + PAGE_SIZE as u64 <= r.end);
    let Some(madt) = find_rsdp(kenv).and_then(|rsdp| acpi::find_table(&rsdp, b"APIC", &phys_read))
    else {
        return;
    };
    for (slot, b) in MADT_IDENT.iter().zip(madt[10..24].iter()) {
        slot.store(*b, Ordering::Relaxed);
    }
    MADT_FOUND.store(true, Ordering::Relaxed);
    if !tramp_ok {
        return;
    }
    let mut n = 1;
    for cpu in acpi::madt_cpus(madt).filter(|c| c.apic_id != bsp) {
        if n == MAXCPU {
            break;
        }
        CPU_APIC_ID[n].store(cpu.apic_id, Ordering::Relaxed);
        n += 1;
    }
    MP_NCPUS.store(n as u32, Ordering::Relaxed);
}

/// `mdi::Smp::cpu_mp_probe`.
pub(crate) fn cpu_mp_probe() -> u32 {
    MP_NCPUS.load(Ordering::Relaxed)
}

/// `mdi::Smp::cpu_apic_id`.
pub(crate) fn cpu_apic_id(cpuid: u32) -> u32 {
    CPU_APIC_ID[cpuid as usize].load(Ordering::Relaxed)
}

/// The MADT's OEM id and OEM table id (6 + 8 bytes), if one was found.
pub(crate) fn madt_ident() -> Option<[u8; 14]> {
    MADT_FOUND
        .load(Ordering::Relaxed)
        .then(|| core::array::from_fn(|i| MADT_IDENT[i].load(Ordering::Relaxed)))
}

/// `mdi::Smp::cpu_topology`: packages, cores per package, threads per
/// core, from the APIC ids and CPUID leaf 0xB.
pub(crate) fn cpu_topology() -> (u32, u32, u32) {
    let n = cpu_mp_probe() as usize;
    let Some((smt, core)) = topo_shifts() else {
        return (1, n as u32, 1);
    };
    let ids = || (0..n).map(|i| CPU_APIC_ID[i].load(Ordering::Relaxed));
    let distinct = |shift: u32| {
        let mut count = 0;
        for (i, a) in ids().enumerate() {
            if ids().take(i).all(|b| b >> shift != a >> shift) {
                count += 1;
            }
        }
        count
    };
    let packages = distinct(core).max(1);
    let cores = distinct(smt).max(1);
    (packages, cores / packages, (n as u32 / cores).max(1))
}

/// Writes a word of the trampoline copy at the offset `field` has in the
/// original.
fn mptramp_set(field: *const u8, value: u64) {
    let off = field as usize - &raw const mptramp_start as usize;
    // SAFETY: the trampoline page is RAM reserved for it (outside
    // phys_avail), reached through the direct map; `off` is inside the
    // copied trampoline and 8-byte aligned (`.balign 8` in mpboot.rs).
    unsafe { (phys_to_dmap(MPTRAMP_ADDR + off as u64) as *mut u64).write_volatile(value) };
}

/// `mdi::Smp::cpu_start_ap` (`start_ap`).
///
/// # Safety
///
/// See `mdi::Smp::cpu_start_ap`.
pub(crate) unsafe fn cpu_start_ap(cpuid: u32, stack_top: usize, ist_tops: [usize; 3]) -> bool {
    for (slot, top) in AP_IST.iter().zip(ist_tops) {
        slot.store(top as u64, Ordering::Relaxed);
    }
    let start = &raw const mptramp_start;
    let len = &raw const mptramp_end as usize - start as usize;
    assert!(len <= PAGE_SIZE, "mpboot: trampoline larger than a page");
    // SAFETY: copies the trampoline (kernel rodata) to its reserved page,
    // through the direct map; no AP runs it while the previous one is
    // already in the kernel (starts are serialized).
    unsafe { core::ptr::copy_nonoverlapping(start, phys_to_dmap(MPTRAMP_ADDR) as *mut u8, len) };
    mptramp_set(&raw const mpt_cr3, mptramp_pml4());
    mptramp_set(&raw const mpt_stack, stack_top as u64);
    let entry: extern "C" fn(u64) -> ! = init_secondary;
    mptramp_set(&raw const mpt_entry, entry as usize as u64);
    mptramp_set(&raw const mpt_cpuid, u64::from(cpuid));

    AP_STARTED.store(0, Ordering::Release);
    ipi_startup(cpu_apic_id(cpuid), (MPTRAMP_ADDR >> 12) as u8);
    for _ in 0..AP_START_TIMEOUT_MS {
        if AP_STARTED.load(Ordering::Acquire) == cpuid {
            return true;
        }
        delay(1000);
    }
    false
}

unsafe extern "Rust" {
    /// The framework's AP entry (`mdi::ApStartupUpcall`).
    fn __nova_ap_startup(cpuid: u32) -> !;
}

/// An AP's first kernel code, entered from the trampoline in long mode,
/// on the trampoline's page tables, with interrupts disabled
/// (`init_secondary`).
extern "C" fn init_secondary(cpuid: u64) -> ! {
    let cpuid = cpuid as u32;
    // SAFETY: the kernel's page tables map everything the trampoline's do
    // in the kernel half, which is where we run; %cr0/%cr4 become the boot
    // CPU's, which the kernel is built for.
    unsafe {
        load_cr3(kernel_pml4());
        load_cr4(BSP_CR4.load(Ordering::Relaxed));
        load_cr0(BSP_CR0.load(Ordering::Relaxed));
    }
    pcpu_init(cpuid);
    let ist = core::array::from_fn(|i| AP_IST[i].load(Ordering::Relaxed));
    cpu_tables_init(cpuid, ist);
    if let Err(e) = lapic_init(APIC_SPURIOUS_INT) {
        panic!("init_secondary: local APIC: {e:?}");
    }
    AP_STARTED.store(cpuid, Ordering::Release);
    // SAFETY: defined once by the framework with the type of
    // `mdi::ApStartupUpcall` (checked there); this CPU is set up and has
    // interrupts disabled, on the stack the framework gave it.
    unsafe { __nova_ap_startup(cpuid) }
}
