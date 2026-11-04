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

use alloc::collections::BTreeMap;
use alloc::sync::Arc;

use mdi::UserMode;

use crate::cpu::{self, intr_guard};
use crate::mm::Frame;
use crate::sync::MtxSpin;
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

struct Page {
    frame: Frame,
    prot: Prot,
}

/// A user address space.
pub struct VmSpace {
    root: u64,
    /// Mapped pages by user address. The lock also serializes changes to
    /// the page tables of `root`.
    pages: MtxSpin<BTreeMap<u64, Page>>,
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

impl VmSpace {
    /// An empty address space (`vmspace_alloc`), or `None` without memory.
    pub fn new() -> Option<Arc<Self>> {
        let root = Md::pmap_pinit(&mut pt_alloc)?;
        Some(Arc::new(Self {
            root,
            pages: MtxSpin::new(BTreeMap::new()),
        }))
    }

    fn check(va: u64, len: u64) -> Result<(), VmError> {
        let page = PAGE_SIZE as u64;
        match va.checked_add(len) {
            Some(end) if va.is_multiple_of(page) && len.is_multiple_of(page) && end <= VM_MAXUSER_ADDRESS => {
                Ok(())
            }
            _ => Err(VmError::Invalid),
        }
    }

    /// Maps `[va, va + len)` to fresh zeroed pages with `prot`
    /// (`vm_map_find` of anonymous memory). Both must be page aligned.
    pub fn map_zeroed(&self, va: u64, len: u64, prot: Prot) -> Result<(), VmError> {
        Self::check(va, len)?;
        let mut pages = self.pages.lock();
        if pages.range(va..va + len).next().is_some() {
            return Err(VmError::Exists);
        }
        for page in (va..va + len).step_by(PAGE_SIZE) {
            let frame = Frame::alloc().ok_or(VmError::NoMemory)?;
            // SAFETY: `root` is live (we own it); `page` is aligned, in user
            // space and unmapped (checked above, under the lock that
            // serializes changes to `root`); the frame is kept in `pages`
            // for as long as the mapping exists, i.e. until `drop`.
            if !unsafe { Md::pmap_enter_user(self.root, page, frame.paddr(), prot, &mut pt_alloc) } {
                return Err(VmError::NoMemory);
            }
            pages.insert(page, Page { frame, prot });
        }
        Ok(())
    }

    /// Calls `f(page, offset, chunk_len)` for each page piece of
    /// `[va, va + len)`, or fails with `Fault` before calling it at all if
    /// a page is missing or `need_write` and not writable.
    fn each_page(
        pages: &mut BTreeMap<u64, Page>,
        va: u64,
        len: usize,
        need_write: bool,
        mut f: impl FnMut(&mut Page, usize, usize, usize),
    ) -> Result<(), VmError> {
        let end = va.checked_add(len as u64).filter(|&e| e <= VM_MAXUSER_ADDRESS).ok_or(VmError::Fault)?;
        let mask = PAGE_SIZE as u64 - 1;
        let mut a = va & !mask;
        while a < end {
            match pages.get(&a) {
                Some(p) if !need_write || p.prot.write => {}
                _ => return Err(VmError::Fault),
            }
            a += PAGE_SIZE as u64;
        }
        let mut a = va;
        let mut done = 0;
        while a < end {
            let page = a & !mask;
            let off = (a - page) as usize;
            let n = (PAGE_SIZE - off).min((end - a) as usize);
            f(pages.get_mut(&page).expect("checked above"), off, done, n);
            a += n as u64;
            done += n;
        }
        Ok(())
    }

    /// Copies user memory at `va` into `buf` (`copyin`).
    pub fn copyin(&self, va: u64, buf: &mut [u8]) -> Result<(), VmError> {
        let mut pages = self.pages.lock();
        Self::each_page(&mut pages, va, buf.len(), false, |p, off, done, n| {
            p.frame.read_bytes(off, &mut buf[done..done + n]);
        })
    }

    /// Copies `buf` to user memory at `va` (`copyout`); fails on pages the
    /// program may not write.
    pub fn copyout(&self, va: u64, buf: &[u8]) -> Result<(), VmError> {
        self.write(va, buf, true)
    }

    /// Copies `buf` to user memory at `va` whatever the pages' protection:
    /// for the kernel filling in an image it is building (`exec`).
    pub fn copyout_init(&self, va: u64, buf: &[u8]) -> Result<(), VmError> {
        self.write(va, buf, false)
    }

    fn write(&self, va: u64, buf: &[u8], need_write: bool) -> Result<(), VmError> {
        let mut pages = self.pages.lock();
        Self::each_page(&mut pages, va, buf.len(), need_write, |p, off, done, n| {
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
        // holds an `Arc` to it (see `task`), so the last one is gone. The
        // pages it mapped are freed after it, with `pages`.
        unsafe { Md::pmap_release(self.root, &mut free) };
    }
}

/// Makes `vm` the address space of the current thread (FreeBSD's
/// `vmspace_switch_aio`/`exec` switch), replacing any previous one.
pub fn set_vmspace(vm: Option<Arc<VmSpace>>) {
    task::set_current_vmspace(vm);
}

/// Activates `vm` (or the kernel's space alone) on this CPU. Called by
/// the scheduler with interrupts disabled.
pub(crate) fn activate(vm: Option<&VmSpace>) {
    // SAFETY: the space is live, and stays live while active here: the
    // caller keeps an `Arc` to it in this CPU's state until another space
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
