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
//! After the bootstrap, the framework maps single pages in the kernel's
//! own range (`KVA_MIN..KVA_MAX`, FreeBSD's kernel map in PML4 slot
//! `KPML4BASE`) with `pmap_kenter`/`pmap_kremove`; kernel stacks with their
//! guard pages live there. Page tables are reached through the direct map.
//!
//! A user address space (`pmap_pinit`) is a PML4 whose lower half is its
//! own and whose upper half is a copy of the kernel's. Every kernel PML4
//! entry exists from the bootstrap on and is never replaced, so the copies
//! never go stale: the kernel map grows below them, in shared pages.
//!
//! TODO(vm): PG_G for kernel pages, NX, PCID, and TLB shootdown IPIs (smp).

use libkern::conv::{u64_from_usize, usize_from_u64};
use core::sync::atomic::{AtomicU64, Ordering};

use mdi::PhysRange;
use x86::cpufunc::{invlpg, load_cr3, rcr3};
use x86::identcpu::has_page1gb;

use crate::param::PAGE_SIZE;
use crate::vmparam::{
    BOOT_MAPPED, DMAP_MIN_ADDRESS, DMPML4I, KERNBASE, KPDPI, KPML4I, NDMPML4E, VM_MAX_KERNEL_MAP,
    VM_MIN_KERNEL_ADDRESS,
};

/// Present.
const PG_V: u64 = 0x001;
/// Writable.
const PG_RW: u64 = 0x002;
/// Accessible from user mode.
const PG_U: u64 = 0x004;
/// Large page (2 MiB in a PD, 1 GiB in a PDP).
const PG_PS: u64 = 0x080;
/// Physical frame bits of an entry.
const PG_FRAME: u64 = 0x000f_ffff_ffff_f000;

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

/// Physical address of the kernel's PML4, shared by all CPUs.
static KERNEL_PML4: AtomicU64 = AtomicU64::new(0);
/// Physical address of the AP trampoline's PML4 (below 4 GiB): the
/// kernel's mappings plus an identity map of the first 2 MiB, where the
/// trampoline runs while it switches to long mode.
static MPTRAMP_PML4: AtomicU64 = AtomicU64::new(0);

/// The kernel's PML4.
pub(crate) fn kernel_pml4() -> u64 {
    KERNEL_PML4.load(Ordering::Relaxed)
}

/// The trampoline's PML4, with the kernel's top-level entries copied in
/// afresh (the kernel map may have grown since boot).
pub(crate) fn mptramp_pml4() -> u64 {
    let tramp = MPTRAMP_PML4.load(Ordering::Relaxed);
    let (src, dst) = (dmap_table(kernel_pml4()), dmap_table(tramp));
    for i in usize_from_u64(NPTEPG) / 2..usize_from_u64(NPTEPG) {
        // SAFETY: both are page-table pages reached through the direct map;
        // the trampoline's is used by no CPU while it is being refreshed
        // (APs are started one at a time).
        unsafe { dst.add(i).write(src.add(i).read()) };
    }
    tramp
}

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
    usize_from_u64(DMAP_MIN_ADDRESS + pa)
}

/// `DMAP_TO_PHYS`.
pub(crate) fn dmap_to_phys(va: usize) -> u64 {
    let va = u64_from_usize(va);
    assert!(
        va >= DMAP_MIN_ADDRESS && va - DMAP_MIN_ADDRESS < dmap_limit(),
        "DMAP_TO_PHYS: {va:#x} is not in the direct map"
    );
    va - DMAP_MIN_ADDRESS
}

/// Takes `n` zeroed pages at `*physfree` (FreeBSD's `allocpages`).
fn allocpages(physfree: &mut u64, n: u64) -> u64 {
    let pa = *physfree;
    *physfree += n * u64_from_usize(PAGE_SIZE);
    assert!(
        *physfree <= BOOT_MAPPED,
        "pmap_bootstrap: out of boot-mapped memory"
    );
    // SAFETY: [pa, physfree) is free RAM right after the loader's data, below
    // BOOT_MAPPED, so the loader maps it at KERNBASE and nothing uses it.
    unsafe { core::ptr::write_bytes(core::ptr::with_exposed_provenance_mut::<u8>(usize_from_u64(KERNBASE + pa)), 0, usize_from_u64(n) * PAGE_SIZE) };
    pa
}

/// A page-table page, through the loader's `KERNBASE` mapping.
fn table(pa: u64) -> &'static mut [u64; usize_from_u64(NPTEPG)] {
    // SAFETY: `pa` comes from `allocpages`: a page below BOOT_MAPPED, owned
    // by the page tables under construction, and only reached through the
    // reference returned here while they are built.
    unsafe { &mut *core::ptr::with_exposed_provenance_mut::<[u64; usize_from_u64(NPTEPG)]>(usize_from_u64(KERNBASE + pa)) }
}

/// Builds and loads the kernel's page tables. `physfree` is the first free
/// physical page after the loader's data and is advanced past the
/// page-table pages. Returns the end of the direct map's physical range.
pub(crate) fn pmap_bootstrap(physfree: &mut u64, physmem: &[PhysRange]) -> u64 {
    *physfree = physfree.next_multiple_of(u64_from_usize(PAGE_SIZE));
    let top = physmem.last().map_or(0, |r| r.end);
    let max = u64_from_usize(NDMPML4E) * NPTEPG * NBPDP;
    let limit = top.max(DMAP_MIN_SIZE).next_multiple_of(NBPDP).min(max);
    let ndmpdp = limit / NBPDP;

    let pml4_pa = allocpages(physfree, 1);
    let pml4 = table(pml4_pa);

    // The kernel: KERNBASE -> physical [0, 1 GiB), 2 MiB pages.
    let kpdp_pa = allocpages(physfree, 1);
    let kpd_pa = allocpages(physfree, 1);
    for (i, pde) in table(kpd_pa).iter_mut().enumerate() {
        *pde = (u64_from_usize(i) * NBPDR) | PG_V | PG_RW | PG_PS;
    }
    table(kpdp_pa)[KPDPI] = kpd_pa | PG_V | PG_RW;
    pml4[KPML4I] = kpdp_pa | PG_V | PG_RW;

    // The kernel map's PDP page, so that its PML4 entry never changes.
    pml4[pindex(VM_MIN_KERNEL_ADDRESS, 3)] = allocpages(physfree, 1) | PG_V | PG_RW;

    // The direct map: one PDP page per 512 GiB.
    let ndmpml4 = ndmpdp.div_ceil(NPTEPG);
    let dmpdp_pa = allocpages(physfree, ndmpml4);
    let dmpdp =
        |i: u64| &mut table(dmpdp_pa + (i / NPTEPG) * u64_from_usize(PAGE_SIZE))[usize_from_u64(i % NPTEPG)];
    if has_page1gb() {
        for i in 0..ndmpdp {
            *dmpdp(i) = (i * NBPDP) | PG_V | PG_RW | PG_PS;
        }
    } else {
        let dmpd_pa = allocpages(physfree, ndmpdp);
        for i in 0..ndmpdp {
            let pd_pa = dmpd_pa + i * u64_from_usize(PAGE_SIZE);
            for (j, pde) in table(pd_pa).iter_mut().enumerate() {
                *pde = (i * NBPDP + u64_from_usize(j) * NBPDR) | PG_V | PG_RW | PG_PS;
            }
            *dmpdp(i) = pd_pa | PG_V | PG_RW;
        }
    }
    for i in 0..ndmpml4 {
        pml4[DMPML4I + usize_from_u64(i)] = (dmpdp_pa + i * u64_from_usize(PAGE_SIZE)) | PG_V | PG_RW;
    }

    // The AP trampoline's tables: identity-map [0, 2 MiB) in the low half;
    // the kernel half is copied in by `mptramp_pml4` at AP start.
    let tramp = allocpages(physfree, 3);
    let (tpdp, tpd) = (tramp + u64_from_usize(PAGE_SIZE), tramp + 2 * u64_from_usize(PAGE_SIZE));
    table(tpd)[0] = PG_V | PG_RW | PG_PS;
    table(tpdp)[0] = tpd | PG_V | PG_RW;
    table(tramp)[0] = tpdp | PG_V | PG_RW;
    MPTRAMP_PML4.store(tramp, Ordering::Relaxed);

    // SAFETY: the new tables map the kernel image, its stacks and the
    // loader's data at the same KERNBASE addresses as the loader's tables,
    // so execution continues unchanged; the page-table pages themselves
    // stay reachable through KERNBASE (they are below BOOT_MAPPED).
    unsafe { load_cr3(pml4_pa) };
    KERNEL_PML4.store(pml4_pa, Ordering::Relaxed);
    DMAP_LIMIT.store(limit, Ordering::Relaxed);
    limit
}

/// Index of `va` in the page table at `level` (3 = PML4, 0 = PT).
fn pindex(va: usize, level: u32) -> usize {
    (va >> (12 + 9 * level)) & 0x1ff
}

/// A page-table page, through the direct map.
fn dmap_table(pa: u64) -> *mut u64 {
    core::ptr::with_exposed_provenance_mut::<u64>(phys_to_dmap(pa & PG_FRAME))
}

/// `mdi::KernelMap::pmap_kenter`.
///
/// # Safety
///
/// See `mdi::KernelMap::pmap_kenter`.
pub(crate) unsafe fn pmap_kenter(
    va: usize,
    pa: u64,
    alloc: &mut dyn FnMut() -> Option<u64>,
) -> bool {
    assert!(
        (VM_MIN_KERNEL_ADDRESS..VM_MAX_KERNEL_MAP).contains(&va) && va.is_multiple_of(PAGE_SIZE)
    );
    let mut table = dmap_table(kernel_pml4());
    for level in (1..=3).rev() {
        // SAFETY: `table` is a live page-table page (the PML4, or one
        // reached from it), mapped by the direct map; the caller serializes
        // calls, so no one else modifies it.
        let entry = unsafe { &mut *table.add(pindex(va, level)) };
        if *entry & PG_V == 0 {
            let Some(pt_pa) = alloc() else { return false };
            *entry = pt_pa | PG_V | PG_RW;
        }
        assert!(
            *entry & PG_PS == 0,
            "pmap_kenter: large page in the kernel map"
        );
        table = dmap_table(*entry);
    }
    // SAFETY: as above, for the last-level table.
    let pte = unsafe { &mut *table.add(pindex(va, 0)) };
    assert!(*pte & PG_V == 0, "pmap_kenter: {va:#x} already mapped");
    *pte = (pa & PG_FRAME) | PG_V | PG_RW;
    true
}

/// `mdi::KernelMap::pmap_kremove`.
///
/// # Safety
///
/// See `mdi::KernelMap::pmap_kremove`.
pub(crate) unsafe fn pmap_kremove(va: usize) {
    let mut table = dmap_table(kernel_pml4());
    for level in (1..=3).rev() {
        // SAFETY: as in `pmap_kenter`; the caller says `va` is mapped, so
        // every level exists.
        let entry = unsafe { *table.add(pindex(va, level)) };
        assert!(
            entry & PG_V != 0 && entry & PG_PS == 0,
            "pmap_kremove: {va:#x} not mapped"
        );
        table = dmap_table(entry);
    }
    // SAFETY: as above.
    unsafe { table.add(pindex(va, 0)).write(0) };
    // TODO(smp): shoot the entry down on the other CPUs too.
    invlpg(va);
}

/// `mdi::KernelMap::pmap_kextract`: walks the current page tables through
/// the direct map, so it never faults.
pub(crate) fn pmap_kextract(va: usize) -> Option<u64> {
    if dmap_limit() == 0 {
        return None; // still on the loader's tables
    }
    let mut table = dmap_table(rcr3());
    for level in (0..=3).rev() {
        // SAFETY: page-table pages are RAM, inside the direct map; reading
        // an entry has no side effects.
        let entry = unsafe { table.add(pindex(va, level)).read_volatile() };
        if entry & PG_V == 0 {
            return None;
        }
        if level == 0 || (level <= 2 && entry & PG_PS != 0) {
            let page_mask = (1u64 << (12 + 9 * level)) - 1;
            return Some((entry & PG_FRAME & !page_mask) | (u64_from_usize(va) & page_mask));
        }
        table = dmap_table(entry);
    }
    None
}

/// First PML4 slot of the kernel half.
const NUPML4E: usize = usize_from_u64(NPTEPG) / 2;

/// `mdi::UserMode::pmap_pinit`: a new, empty user address space.
pub(crate) fn pmap_pinit(alloc: &mut dyn FnMut() -> Option<u64>) -> Option<u64> {
    let root = alloc()?;
    let (src, dst) = (dmap_table(kernel_pml4()), dmap_table(root));
    for i in 0..usize_from_u64(NPTEPG) {
        let e = if i < NUPML4E { 0 } else {
            // SAFETY: the kernel's PML4, through the direct map; its upper
            // half is fixed after the bootstrap.
            unsafe { src.add(i).read() }
        };
        // SAFETY: `root` is a fresh page from `alloc`, ours alone.
        unsafe { dst.add(i).write(e) };
    }
    Some(root)
}

/// `mdi::UserMode::pmap_enter_user`.
///
/// # Safety
///
/// See `mdi::UserMode::pmap_enter_user`.
pub(crate) unsafe fn pmap_enter_user(
    root: u64,
    va: u64,
    pa: u64,
    prot: mdi::Prot,
    alloc: &mut dyn FnMut() -> Option<u64>,
) -> bool {
    assert!(va < crate::usermode::VM_MAXUSER_ADDRESS && va.is_multiple_of(u64_from_usize(PAGE_SIZE)));
    let va = usize_from_u64(va);
    let mut table = dmap_table(root);
    for level in (1..=3).rev() {
        // SAFETY: `table` is a page-table page of `root`'s user half,
        // through the direct map; the caller serializes changes to `root`.
        let entry = unsafe { &mut *table.add(pindex(va, level)) };
        if *entry & PG_V == 0 {
            let Some(pt_pa) = alloc() else { return false };
            *entry = pt_pa | PG_V | PG_RW | PG_U;
        }
        table = dmap_table(*entry);
    }
    // SAFETY: as above, for the last-level table.
    let pte = unsafe { &mut *table.add(pindex(va, 0)) };
    let rw = if prot.write { PG_RW } else { 0 };
    let old = *pte;
    *pte = (pa & PG_FRAME) | PG_V | PG_U | rw;
    if old & PG_V != 0 && rcr3() & PG_FRAME == root {
        // TODO(smp): other CPUs running this address space.
        invlpg(va);
    }
    true
}

/// `mdi::UserMode::pmap_remove_user`.
///
/// # Safety
///
/// See `mdi::UserMode::pmap_remove_user`.
pub(crate) unsafe fn pmap_remove_user(root: u64, va: u64) -> Option<u64> {
    if va >= crate::usermode::VM_MAXUSER_ADDRESS || !va.is_multiple_of(u64_from_usize(PAGE_SIZE)) {
        return None;
    }
    let va = usize_from_u64(va);
    let mut table = dmap_table(root);
    for level in (1..=3).rev() {
        // SAFETY: a page-table page of `root`'s user half, through the
        // direct map; the caller serializes changes to `root`.
        let entry = unsafe { table.add(pindex(va, level)).read() };
        if entry & PG_V == 0 {
            return None;
        }
        table = dmap_table(entry);
    }
    // SAFETY: as above, for the last-level table.
    let pte = unsafe { &mut *table.add(pindex(va, 0)) };
    let old = *pte;
    if old & PG_V == 0 {
        return None;
    }
    *pte = 0;
    if rcr3() & PG_FRAME == root {
        invlpg(va);
    }
    Some(old & PG_FRAME)
}

/// `mdi::UserMode::pmap_release`: frees the user half's page-table pages
/// and the root (not the pages they map: the caller owns those).
///
/// # Safety
///
/// See `mdi::UserMode::pmap_release`.
pub(crate) unsafe fn pmap_release(root: u64, free: &mut dyn FnMut(u64)) {
    fn release(pa: u64, level: u32, n: usize, free: &mut dyn FnMut(u64)) {
        if level > 0 {
            let table = dmap_table(pa);
            for i in 0..n {
                // SAFETY: a page-table page of the released space, which no
                // CPU uses any more (the caller's contract).
                let e = unsafe { table.add(i).read() };
                if e & PG_V != 0 {
                    release(e & PG_FRAME, level - 1, usize_from_u64(NPTEPG), free);
                }
            }
        }
        free(pa);
    }
    // Level-0 tables hold leaf entries, the caller's pages: release(_, 0)
    // frees only the table page itself.
    release(root, 3, NUPML4E, free);
}

/// `mdi::UserMode::pmap_activate`.
///
/// # Safety
///
/// See `mdi::UserMode::pmap_activate`.
pub(crate) unsafe fn pmap_activate(root: u64) {
    let root = if root == 0 { kernel_pml4() } else { root };
    // SAFETY: `root` is the kernel's PML4 or one made by `pmap_pinit`, whose
    // kernel half is the kernel's: the kernel keeps running unchanged.
    unsafe { load_cr3(root) };
}
