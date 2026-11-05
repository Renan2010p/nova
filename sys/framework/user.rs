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

//! User mode: address spaces and running a thread in user mode.
//!
//! * [`VmSpace`]: a user address space (the mechanism half of FreeBSD's
//!   `struct vmspace`). It owns every page it maps; the kernel reaches
//!   them only by copying ([`VmSpace::copyin`], [`VmSpace::copyout`]),
//!   never through a user pointer, so no Rust reference ever points into
//!   memory a program can change;
//! * [`set_vmspace`] gives the current thread an address space, which the
//!   scheduler then activates whenever the thread runs;
//! * [`enter`] runs the current thread in user mode until it comes back
//!   with a system call, an interrupt or a fault.
//!
//! Nothing here knows an ABI: [`UserRegs`] is the machine's register file,
//! and what a system call is or which signal a fault means belongs to the
//! ABIs in `sys/compat`.

use core::sync::atomic::{AtomicU64, Ordering};

use libkern::conv::{u64_from_usize, usize_from_u64};
use libkern::range::{page_range_end, range_end};
use mdi::UserMode;

use crate::cpu::{self, intr_guard};
use crate::mm::{AllocError, Frame, KArc, KVec, M_WAITOK, MallocType};
use crate::sync::Mutex;
use crate::{Md, PAGE_SIZE, task};

pub use mdi::{FaultKind, Prot, UserExit, UserFault};

/// The machine's user register file (opaque to MI code; ABIs read it in
/// their `arch/<arch>/` half).
pub type UserRegs = <Md as UserMode>::UserRegs;

/// First address above user space.
pub const VM_MAXUSER_ADDRESS: u64 = <Md as UserMode>::VM_MAXUSER_ADDRESS;

/// Why a [`VmSpace`] operation failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VmError {
    /// Out of physical memory.
    NoMemory,
    /// The range is not page aligned, wraps, or is outside user space.
    Invalid,
    /// A page of the range is already mapped.
    Exists,
    /// A page of the range is not mapped (or not writable, for
    /// [`VmSpace::copyout`]): FreeBSD's `EFAULT`.
    Fault,
}

impl From<AllocError> for VmError {
    fn from(_: AllocError) -> Self {
        VmError::NoMemory
    }
}

/// Address spaces and their page lists (`M_VMSPACE`... FreeBSD's
/// `vmspace` zone).
static M_VMSPACE: MallocType = MallocType::new("vmspace");

/// Where [`VmSpace::map_anon`] puts a mapping.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MapPlace {
    /// The lowest free range at or above the address.
    Hint {
        /// The start is aligned to `2^align_shift` bytes (at least a page).
        align_shift: u32,
    },
    /// Exactly at the address, replacing what was there (`MAP_FIXED`).
    Fixed,
    /// Exactly at the address, failing with `Exists` if anything is there
    /// (`MAP_FIXED | MAP_EXCL`).
    FixedNoReplace,
}

/// What a child gets of a region at `fork` (`vm_inherit_t`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Inherit {
    /// The same memory, shared (`VM_INHERIT_SHARE`).
    Share,
    /// A copy (`VM_INHERIT_COPY`), the default.
    #[default]
    Copy,
    /// Nothing: the range is unmapped in the child (`VM_INHERIT_NONE`).
    None,
    /// Fresh zeroed memory (`VM_INHERIT_ZERO`), e.g. a random generator's
    /// state, so parent and child never repeat each other.
    Zero,
}

/// A range of the address space that may be used (FreeBSD's
/// `vm_map_entry`): pages in it are allocated when first touched.
#[derive(Clone, Copy, Debug)]
struct Region {
    start: u64,
    end: u64,
    prot: Prot,
    inherit: Inherit,
}

/// A page that is present.
struct Page {
    va: u64,
    frame: Frame,
    /// In the page tables; false while its region may not be read
    /// (`PROT_NONE`), when touching it must fault.
    mapped: bool,
}

/// The map of a space: its regions and its present pages, both sorted by
/// address, with every page inside a region.
struct Map {
    regions: KVec<Region>,
    pages: KVec<Page>,
}

/// A user address space.
pub struct VmSpace {
    root: u64,
    /// Under a sleep lock, as FreeBSD's `vm_map` lock: it is held while
    /// allocating. It also serializes changes to the page tables of `root`.
    map: Mutex<Map>,
    /// Bumped whenever a page is unmapped: a CPU that last loaded this
    /// space at an older generation may hold stale TLB entries and reloads
    /// it before running it again.
    generation: AtomicU64,
}

impl core::fmt::Debug for VmSpace {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("VmSpace").field("root", &self.root).finish_non_exhaustive()
    }
}

/// A zeroed page for the page tables, given up until `pmap_release`.
fn pt_alloc() -> Option<u64> {
    Frame::alloc().map(Frame::leak)
}

const PAGE: u64 = u64_from_usize(PAGE_SIZE);

impl Map {
    fn find_page(&self, va: u64) -> Result<usize, usize> {
        self.pages.binary_search_by_key(&va, |p| p.va)
    }

    /// The region containing `va`.
    fn region(&self, va: u64) -> Option<&Region> {
        let i = self.regions.partition_point(|r| r.end <= va);
        self.regions.get(i).filter(|r| r.start <= va)
    }

    /// True if any region overlaps `[start, end)`.
    fn overlaps(&self, start: u64, end: u64) -> bool {
        let i = self.regions.partition_point(|r| r.end <= start);
        self.regions.get(i).is_some_and(|r| r.start < end)
    }

    fn add_region(&mut self, r: Region) -> Result<(), VmError> {
        let i = self.regions.partition_point(|x| x.end <= r.start);
        self.regions.insert(i, r, M_WAITOK)?;
        Ok(())
    }
}

impl VmSpace {
    /// An empty address space (`vmspace_alloc`). May sleep.
    pub fn new() -> Result<KArc<Self>, AllocError> {
        let root = Md::pmap_pinit(&mut pt_alloc).ok_or(AllocError)?;
        let vm = Self {
            root,
            map: Mutex::new(Map { regions: KVec::new(&M_VMSPACE), pages: KVec::new(&M_VMSPACE) }),
            generation: AtomicU64::new(0),
        };
        // On failure `vm` is dropped, which releases `root`.
        KArc::new(vm, &M_VMSPACE, M_WAITOK)
    }

    /// The generation of the page tables (see `generation`).
    pub(crate) fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    fn check(va: u64, len: u64) -> Result<u64, VmError> {
        page_range_end(va, len, VM_MAXUSER_ADDRESS, PAGE).ok_or(VmError::Invalid)
    }

    /// Makes `va` present in `map`: allocates, zeroes and maps its page.
    fn page_in(&self, map: &mut Map, va: u64, prot: Prot) -> Result<usize, VmError> {
        let at = match map.find_page(va) {
            Ok(i) => return Ok(i),
            Err(at) => at,
        };
        map.pages.reserve(1, M_WAITOK)?;
        let frame = Frame::alloc().ok_or(VmError::NoMemory)?;
        // SAFETY: `root` is live (we own it); `va` is aligned, in user space
        // and not mapped (not in `pages`), under the lock that serializes
        // changes to `root`; the frame stays in `pages` as long as it is
        // mapped.
        if !unsafe { Md::pmap_enter_user(self.root, va, frame.paddr(), prot, &mut pt_alloc) } {
            return Err(VmError::NoMemory);
        }
        // Room was reserved: this cannot fail after the page is mapped.
        map.pages.insert(at, Page { va, frame, mapped: true }, M_WAITOK)?;
        Ok(at)
    }

    /// Maps `[va, va + len)` to fresh zeroed pages with `prot`, present at
    /// once (for an image being built: `exec`). May sleep.
    pub fn map_zeroed(&self, va: u64, len: u64, prot: Prot) -> Result<(), VmError> {
        let end = Self::check(va, len)?;
        let mut map = self.map.lock();
        if map.overlaps(va, end) {
            return Err(VmError::Exists);
        }
        map.add_region(Region { start: va, end, prot, inherit: Inherit::Copy })?;
        for page in (va..end).step_by(PAGE_SIZE) {
            self.page_in(&mut map, page, prot)?;
        }
        Ok(())
    }

    /// Reserves `len` bytes of anonymous memory with `prot`, whose pages
    /// are allocated and zeroed when first touched (`vm_map_find`). With
    /// `fixed`, exactly at `addr`, replacing what was there (`MAP_FIXED`);
    /// otherwise at the lowest free range at or above `addr`. Returns the
    /// address. May sleep.
    pub fn map_anon(&self, addr: u64, len: u64, prot: Prot, place: MapPlace) -> Result<u64, VmError> {
        if len == 0 || !addr.is_multiple_of(PAGE) {
            return Err(VmError::Invalid);
        }
        let len = len.checked_next_multiple_of(PAGE).ok_or(VmError::Invalid)?;
        let align_shift = match place {
            MapPlace::Fixed => {
                let end = Self::check(addr, len)?;
                self.unmap(addr, len)?;
                self.map.lock().add_region(Region { start: addr, end, prot, inherit: Inherit::Copy })?;
                return Ok(addr);
            }
            MapPlace::FixedNoReplace => {
                let end = Self::check(addr, len)?;
                let mut map = self.map.lock();
                if map.overlaps(addr, end) {
                    return Err(VmError::Exists);
                }
                map.add_region(Region { start: addr, end, prot, inherit: Inherit::Copy })?;
                return Ok(addr);
            }
            MapPlace::Hint { align_shift } => align_shift,
        };
        let align = 1u64.checked_shl(align_shift).ok_or(VmError::Invalid)?.max(PAGE);
        let mut map = self.map.lock();
        let up = |a: u64| a.checked_next_multiple_of(align).ok_or(VmError::NoMemory);
        let mut start = up(addr.max(PAGE))?;
        // First fit: walk the regions above `start` and their gaps.
        let mut i = map.regions.partition_point(|r| r.end <= start);
        loop {
            let end = Self::check(start, len).map_err(|_| VmError::NoMemory)?;
            match map.regions.get(i) {
                Some(r) if r.start < end => {
                    start = up(r.end)?;
                    i += 1;
                }
                _ => break,
            }
        }
        let end = Self::check(start, len)?;
        map.add_region(Region { start, end, prot, inherit: Inherit::Copy })?;
        Ok(start)
    }

    /// Unmaps `[va, va + len)`: the regions are cut and the pages freed
    /// (`vm_map_delete`). Unmapping nothing is not an error. May sleep.
    pub fn unmap(&self, va: u64, len: u64) -> Result<(), VmError> {
        if !va.is_multiple_of(PAGE) {
            return Err(VmError::Invalid);
        }
        let len = len.checked_next_multiple_of(PAGE).ok_or(VmError::Invalid)?;
        let end = Self::check(va, len)?;
        let mut map = self.map.lock();
        // Cut the regions: keep what lies outside [va, end).
        let mut i = map.regions.partition_point(|r| r.end <= va);
        while let Some(&r) = map.regions.get(i) {
            if r.start >= end {
                break;
            }
            map.regions.remove(i);
            if r.start < va {
                map.regions.insert(i, Region { end: va, ..r }, M_WAITOK)?;
                i += 1;
            }
            if r.end > end {
                map.regions.insert(i, Region { start: end, ..r }, M_WAITOK)?;
                i += 1;
            }
        }
        // Free the pages.
        let first = map.pages.partition_point(|p| p.va < va);
        let mut removed = false;
        while map.pages.get(first).is_some_and(|p| p.va < end) {
            let page = map.pages.remove(first);
            if page.mapped {
                // SAFETY: `root` is live and `page.va` was mapped by
                // `page_in`, under this same lock. The frame is freed only
                // after its PTE is gone (here) and this CPU's TLB updated (by
                // the MD layer).
                unsafe { Md::pmap_remove_user(self.root, page.va) };
            }
            removed = true;
            drop(page);
        }
        if removed {
            // TODO(smp): a thread of this space running on another CPU now
            // needs a TLB shootdown; for now a space runs one thread, and
            // other CPUs reload it (see `generation`) before running it.
            self.generation.fetch_add(1, Ordering::AcqRel);
        }
        Ok(())
    }

    /// Applies `f` to the parts of the regions inside `[va, end)`, cutting
    /// regions at `va` and `end` first (the `vm_map_clip_*` dance).
    fn modify_regions(map: &mut Map, va: u64, end: u64, mut f: impl FnMut(&mut Region)) -> Result<(), VmError> {
        let mut i = map.regions.partition_point(|r| r.end <= va);
        while let Some(&r) = map.regions.get(i) {
            if r.start >= end {
                break;
            }
            map.regions.remove(i);
            if r.start < va {
                map.regions.insert(i, Region { end: va, ..r }, M_WAITOK)?;
                i += 1;
            }
            let mut mid = Region { start: r.start.max(va), end: r.end.min(end), ..r };
            f(&mut mid);
            map.regions.insert(i, mid, M_WAITOK)?;
            i += 1;
            if r.end > end {
                map.regions.insert(i, Region { start: end, ..r }, M_WAITOK)?;
                i += 1;
            }
        }
        Ok(())
    }

    fn page_range(va: u64, len: u64) -> Result<u64, VmError> {
        if !va.is_multiple_of(PAGE) {
            return Err(VmError::Invalid);
        }
        let len = len.checked_next_multiple_of(PAGE).ok_or(VmError::Invalid)?;
        Self::check(va, len)
    }

    /// Sets what a child gets of `[va, va + len)` at `fork` (`vm_map_inherit`).
    /// Pages outside every region are ignored, as in FreeBSD.
    pub fn set_inherit(&self, va: u64, len: u64, inherit: Inherit) -> Result<(), VmError> {
        let end = Self::page_range(va, len)?;
        Self::modify_regions(&mut self.map.lock(), va, end, |r| r.inherit = inherit)
    }

    /// Changes the protection of `[va, va + len)` (`vm_map_protect`),
    /// including pages already present. `NoMemory` if part of the range
    /// is not mapped, as FreeBSD's `ENOMEM`.
    pub fn protect(&self, va: u64, len: u64, prot: Prot) -> Result<(), VmError> {
        let end = Self::page_range(va, len)?;
        let mut map = self.map.lock();
        // The whole range must be mapped.
        let mut a = va;
        while a < end {
            let r = map.region(a).ok_or(VmError::NoMemory)?;
            a = r.end;
        }
        Self::modify_regions(&mut map, va, end, |r| r.prot = prot)?;
        let first = map.pages.partition_point(|p| p.va < va);
        let mut i = first;
        while let Some(page) = map.pages.get_mut(i) {
            if page.va >= end {
                break;
            }
            // SAFETY: `root` is live and `page.va` is mapped to `page.frame`
            // (by `page_in`), under this lock; the frame stays owned by
            // `pages`, so remapping it with the new protection is sound. A
            // page that may not be read stays unmapped: touching it faults.
            unsafe {
                Md::pmap_remove_user(self.root, page.va);
                page.mapped = false;
                if prot.read {
                    if !Md::pmap_enter_user(self.root, page.va, page.frame.paddr(), prot, &mut pt_alloc) {
                        return Err(VmError::NoMemory);
                    }
                    page.mapped = true;
                }
            }
            i += 1;
        }
        if i > first {
            self.generation.fetch_add(1, Ordering::AcqRel);
        }
        Ok(())
    }

    /// Resolves a fault at `addr` (`vm_fault`): if it lies in a region
    /// that allows the access, its page is made present. `Fault` if not:
    /// the program touched memory it does not have.
    pub fn fault(&self, addr: u64, write: bool) -> Result<(), VmError> {
        let page = addr & !(PAGE - 1);
        let mut map = self.map.lock();
        let r = *map.region(page).ok_or(VmError::Fault)?;
        if !r.prot.read || (write && !r.prot.write) {
            return Err(VmError::Fault);
        }
        if let Ok(i) = map.find_page(page) {
            // Present: either a genuine protection fault, or a page whose
            // region was made accessible again after `protect` unmapped it.
            let Some(p) = map.pages.get_mut(i) else { return Err(VmError::Fault) };
            if p.mapped {
                return Err(VmError::Fault);
            }
            // SAFETY: as in `page_in`: `root` live, `page` unmapped (not
            // `mapped`), the frame owned by `pages`, under the lock.
            if !unsafe { Md::pmap_enter_user(self.root, page, p.frame.paddr(), r.prot, &mut pt_alloc) } {
                return Err(VmError::NoMemory);
            }
            p.mapped = true;
            return Ok(());
        }
        self.page_in(&mut map, page, r.prot)?;
        Ok(())
    }

    /// Calls `f(page, offset, done, chunk_len)` for each page piece of
    /// `[va, va + len)`, making pages present as needed; fails with
    /// `Fault` before calling it at all if part of the range is outside
    /// every region, or `need_write` and not writable.
    fn each_page(
        &self,
        map: &mut Map,
        va: u64,
        len: usize,
        need_write: bool,
        init: bool,
        mut f: impl FnMut(&mut Page, usize, usize, usize),
    ) -> Result<(), VmError> {
        let end = range_end(va, u64_from_usize(len), VM_MAXUSER_ADDRESS).ok_or(VmError::Fault)?;
        let mut a = va & !(PAGE - 1);
        while a < end {
            let r = *map.region(a).ok_or(VmError::Fault)?;
            if (!r.prot.read && !init) || (need_write && !r.prot.write) {
                return Err(VmError::Fault);
            }
            self.page_in(map, a, r.prot)?;
            a += PAGE;
        }
        let mut a = va;
        let mut done = 0;
        while a < end {
            let page = a & !(PAGE - 1);
            let off = usize_from_u64(a - page);
            let n = (PAGE_SIZE - off).min(usize_from_u64(end - a));
            let i = map.find_page(page).map_err(|_| VmError::Fault)?;
            let p = map.pages.get_mut(i).ok_or(VmError::Fault)?;
            f(p, off, done, n);
            a += u64_from_usize(n);
            done += n;
        }
        Ok(())
    }

    /// A copy of this space for a child process (`vmspace_fork`): each
    /// region as its inheritance says (`minherit`): copied, left out, or
    /// fresh zeroed memory. May sleep.
    ///
    /// TODO(vm): copy-on-write, and `Share` really shared (it is copied).
    pub fn fork(&self) -> Result<KArc<VmSpace>, VmError> {
        let child = VmSpace::new()?;
        {
            let map = self.map.lock();
            let mut cmap = child.map.lock();
            for r in map.regions.iter().filter(|r| r.inherit != Inherit::None) {
                cmap.regions.push(*r, M_WAITOK)?;
            }
            let mut buf = [0u8; 512];
            for page in map.pages.iter() {
                let Some(r) = map.region(page.va) else { continue };
                if !matches!(r.inherit, Inherit::Copy | Inherit::Share) {
                    continue;
                }
                cmap.pages.reserve(1, M_WAITOK)?;
                let mut frame = Frame::alloc().ok_or(VmError::NoMemory)?;
                for off in (0..PAGE_SIZE).step_by(buf.len()) {
                    page.frame.read_bytes(off, &mut buf);
                    frame.write_bytes(off, &buf);
                }
                // As the parent's page: unmapped while its region may not
                // be read.
                let mapped = page.mapped;
                // SAFETY: `child.root` is live (we own it); `page.va` is
                // aligned, in user space and not mapped in the child (pages
                // are copied once each, in order), under the child's map
                // lock; the frame stays in `pages` as long as it is mapped.
                if mapped && !unsafe { Md::pmap_enter_user(child.root, page.va, frame.paddr(), r.prot, &mut pt_alloc) } {
                    return Err(VmError::NoMemory);
                }
                // Room was reserved, and pages come in address order.
                cmap.pages.push(Page { va: page.va, frame, mapped }, M_WAITOK)?;
            }
        }
        Ok(child)
    }

    /// Copies user memory at `va` into `buf` (`copyin`).
    pub fn copyin(&self, va: u64, buf: &mut [u8]) -> Result<(), VmError> {
        let mut map = self.map.lock();
        self.each_page(&mut map, va, buf.len(), false, false, |p, off, done, n| {
            p.frame.read_bytes(off, &mut buf[done..done + n]);
        })
    }

    /// Copies `buf` to user memory at `va` (`copyout`); fails on memory the
    /// program may not write.
    pub fn copyout(&self, va: u64, buf: &[u8]) -> Result<(), VmError> {
        self.write(va, buf, true)
    }

    /// Copies `buf` to user memory at `va` whatever its protection: for
    /// the kernel filling in an image it is building (`exec`).
    pub fn copyout_init(&self, va: u64, buf: &[u8]) -> Result<(), VmError> {
        self.write(va, buf, false)
    }

    fn write(&self, va: u64, buf: &[u8], need_write: bool) -> Result<(), VmError> {
        let mut map = self.map.lock();
        self.each_page(&mut map, va, buf.len(), need_write, !need_write, |p, off, done, n| {
            p.frame.write_bytes(off, &buf[done..done + n]);
        })
    }
}

impl Drop for VmSpace {
    fn drop(&mut self) {
        let mut free = |pa| {
            // SAFETY: a page given up by `pt_alloc` for this space's page
            // tables, which nothing uses any more.
            drop(unsafe { Frame::from_leaked(pa) })
        };
        // SAFETY: no CPU uses `root`: every CPU that activates a space
        // holds a `KArc` to it (see `task`), so the last one is gone. The
        // pages it mapped are freed after it, with `map`.
        unsafe { Md::pmap_release(self.root, &mut free) };
    }
}

/// Makes `vm` the address space of the current thread (FreeBSD's
/// `vmspace_switch_aio`/`exec` switch), replacing any previous one.
pub fn set_vmspace(vm: Option<KArc<VmSpace>>) {
    task::set_current_vmspace(vm);
}

/// Activates `vm` (or the kernel's space alone) on this CPU. Called by
/// the scheduler with interrupts disabled.
pub(crate) fn activate(vm: Option<&VmSpace>) {
    // SAFETY: the space is live, and stays live while active here: the
    // caller keeps a `KArc` to it in this CPU's state until another space
    // is activated.
    unsafe { Md::pmap_activate(vm.map_or(0, |v| v.root)) };
}

/// Runs the current thread in user mode with `regs`, in its address space
/// (see [`set_vmspace`]), until the program comes back to the kernel;
/// `regs` then holds its registers. Interrupts taken meanwhile are handled
/// (and may switch threads) before this returns.
///
/// # Panics
///
/// If an interrupt guard is held, or the thread has no address space.
pub fn enter(regs: &mut UserRegs) -> UserExit {
    cpu::assert_no_guard("user::enter");
    let g = intr_guard();
    assert!(task::activate_current_vmspace(&g), "user::enter: thread has no address space");
    // User mode runs with interrupts on and no guard: interrupts it takes
    // are handled as in the kernel, from depth 0. Drop the guard's count
    // without turning interrupts on; `cpu_user_enter` does that atomically
    // with the switch to user mode.
    cpu::guard_forget(g);
    // SAFETY: interrupts are off and no guard is held; this thread's
    // address space is active on this CPU (just above, and nothing can
    // switch threads before user mode).
    let exit = unsafe { Md::cpu_user_enter(regs) };
    cpu::intr_enable();
    exit
}
