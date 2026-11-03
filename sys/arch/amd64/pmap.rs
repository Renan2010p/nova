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

//! Machine-dependent virtual memory (FreeBSD's `amd64/pmap.c`).
//!
//! For now, only the bootstrap: `pmap_bootstrap` replaces the loader's
//! page tables with the kernel's own (FreeBSD's `create_pagetables`):
//!
//! * the kernel at `KERNBASE`: physical `[0, 1 GiB)` with 2 MiB pages, as
//!   the loader had it, so nothing the loader left behind moves;
//! * the direct map at `DMAP_MIN_ADDRESS`: all of physical memory, at
//!   least 4 GiB, with 1 GiB pages when the CPU has them, else 2 MiB;
//! * nothing else. In particular the low identity mapping is gone, so a
//!   NULL dereference in the kernel now faults.
//!
//! Page-table pages come from right after the loader's data (`physfree`),
//! like FreeBSD's `allocpages`, and are written through the loader's
//! `KERNBASE` mapping before the switch.
//!
//! TODO(vm): `pmap_enter`/`pmap_remove` for kernel and user address
//! spaces, PG_G for kernel pages, NX, PCID, and TLB shootdown IPIs (smp).

use core::sync::atomic::{AtomicU64, Ordering};

use mdi::PhysRange;
use x86::cpufunc::load_cr3;
use x86::identcpu::has_page1gb;

use crate::param::PAGE_SIZE;
use crate::vmparam::{BOOT_MAPPED, DMAP_MIN_ADDRESS, DMPML4I, KERNBASE, KPDPI, KPML4I, NDMPML4E};

/// Present.
const PG_V: u64 = 0x001;
/// Writable.
const PG_RW: u64 = 0x002;
/// Large page (2 MiB in a PD, 1 GiB in a PDP).
const PG_PS: u64 = 0x080;

/// Bytes mapped by one PD entry.
const NBPDR: u64 = 1 << 21;
/// Bytes mapped by one PDP entry.
const NBPDP: u64 = 1 << 30;
/// Entries per page-table page.
const NPTEPG: u64 = 512;

/// The direct map always covers at least the 32-bit physical space, so
/// MMIO below 4 GiB (local APIC, HPET, PCI holes) is reachable.
const DMAP_MIN_SIZE: u64 = 4 * NBPDP;

/// End of the physical range covered by the direct map; 0 until
/// `pmap_bootstrap` has run.
static DMAP_LIMIT: AtomicU64 = AtomicU64::new(0);

/// End of the direct map's physical range.
pub(crate) fn dmap_limit() -> u64 {
    DMAP_LIMIT.load(Ordering::Relaxed)
}

/// `PHYS_TO_DMAP`.
pub(crate) fn phys_to_dmap(pa: u64) -> usize {
    assert!(
        pa < dmap_limit(),
        "PHYS_TO_DMAP: {pa:#x} is not in the direct map"
    );
    (DMAP_MIN_ADDRESS + pa) as usize
}

/// `DMAP_TO_PHYS`.
pub(crate) fn dmap_to_phys(va: usize) -> u64 {
    let va = va as u64;
    assert!(
        va >= DMAP_MIN_ADDRESS && va - DMAP_MIN_ADDRESS < dmap_limit(),
        "DMAP_TO_PHYS: {va:#x} is not in the direct map"
    );
    va - DMAP_MIN_ADDRESS
}

/// Takes `n` zeroed pages at `*physfree` (FreeBSD's `allocpages`).
fn allocpages(physfree: &mut u64, n: u64) -> u64 {
    let pa = *physfree;
    *physfree += n * PAGE_SIZE as u64;
    assert!(
        *physfree <= BOOT_MAPPED,
        "pmap_bootstrap: out of boot-mapped memory"
    );
    // SAFETY: [pa, physfree) is free RAM right after the loader's data, below
    // BOOT_MAPPED, so the loader maps it at KERNBASE and nothing uses it.
    unsafe { core::ptr::write_bytes((KERNBASE + pa) as *mut u8, 0, (n as usize) * PAGE_SIZE) };
    pa
}

/// A page-table page, through the loader's `KERNBASE` mapping.
fn table(pa: u64) -> &'static mut [u64; NPTEPG as usize] {
    // SAFETY: `pa` comes from `allocpages`: a page below BOOT_MAPPED, owned
    // by the page tables under construction, and only reached through the
    // reference returned here while they are built.
    unsafe { &mut *((KERNBASE + pa) as *mut [u64; NPTEPG as usize]) }
}

/// Builds and loads the kernel's page tables. `physfree` is the first free
/// physical page after the loader's data and is advanced past the
/// page-table pages. Returns the end of the direct map's physical range.
pub(crate) fn pmap_bootstrap(physfree: &mut u64, physmem: &[PhysRange]) -> u64 {
    *physfree = physfree.next_multiple_of(PAGE_SIZE as u64);
    let top = physmem.last().map_or(0, |r| r.end);
    let max = NDMPML4E as u64 * NPTEPG * NBPDP;
    let limit = top.max(DMAP_MIN_SIZE).next_multiple_of(NBPDP).min(max);
    let ndmpdp = limit / NBPDP;

    let pml4_pa = allocpages(physfree, 1);
    let pml4 = table(pml4_pa);

    // The kernel: KERNBASE -> physical [0, 1 GiB), 2 MiB pages.
    let kpdp_pa = allocpages(physfree, 1);
    let kpd_pa = allocpages(physfree, 1);
    for (i, pde) in table(kpd_pa).iter_mut().enumerate() {
        *pde = (i as u64 * NBPDR) | PG_V | PG_RW | PG_PS;
    }
    table(kpdp_pa)[KPDPI] = kpd_pa | PG_V | PG_RW;
    pml4[KPML4I] = kpdp_pa | PG_V | PG_RW;

    // The direct map: one PDP page per 512 GiB.
    let ndmpml4 = ndmpdp.div_ceil(NPTEPG);
    let dmpdp_pa = allocpages(physfree, ndmpml4);
    let dmpdp =
        |i: u64| &mut table(dmpdp_pa + (i / NPTEPG) * PAGE_SIZE as u64)[(i % NPTEPG) as usize];
    if has_page1gb() {
        for i in 0..ndmpdp {
            *dmpdp(i) = (i * NBPDP) | PG_V | PG_RW | PG_PS;
        }
    } else {
        let dmpd_pa = allocpages(physfree, ndmpdp);
        for i in 0..ndmpdp {
            let pd_pa = dmpd_pa + i * PAGE_SIZE as u64;
            for (j, pde) in table(pd_pa).iter_mut().enumerate() {
                *pde = (i * NBPDP + j as u64 * NBPDR) | PG_V | PG_RW | PG_PS;
            }
            *dmpdp(i) = pd_pa | PG_V | PG_RW;
        }
    }
    for i in 0..ndmpml4 {
        pml4[DMPML4I + i as usize] = (dmpdp_pa + i * PAGE_SIZE as u64) | PG_V | PG_RW;
    }

    // SAFETY: the new tables map the kernel image, its stacks and the
    // loader's data at the same KERNBASE addresses as the loader's tables,
    // so execution continues unchanged; the page-table pages themselves
    // stay reachable through KERNBASE (they are below BOOT_MAPPED).
    unsafe { load_cr3(pml4_pa) };
    DMAP_LIMIT.store(limit, Ordering::Relaxed);
    limit
}
