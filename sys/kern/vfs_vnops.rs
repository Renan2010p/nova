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


//! Mapping files into memory (`vn_mmap`, FreeBSD's `vm/vm_mmap.c` for
//! vnodes): the region is backed by the page cache (`vnode_pager`); pages
//! are read when touched and shared until written, when a private
//! mapping gets its own copy.
//!
//! TODO(vm): writable shared mappings (written back to the file).

use framework::PAGE_SIZE;
use framework::user::Prot;
use libkern::conv::u64_from_usize;
use sys::errno::{Errno, KResult};
use vm::vm_mmap::{MmapAt, kern_mmap_anon};

use crate::kern_descrip::FileKind;
use crate::kern_proc::Proc;
use crate::vnode::VType;

/// Maps `len` bytes of the file open as `fd`, from `offset`, with `prot`,
/// `shared` or private (`kern_mmap` with a descriptor); returns the
/// address. `/dev/zero` maps anonymous memory.
pub fn kern_mmap_fd(p: &Proc, at: MmapAt, len: u64, prot: Prot, shared: bool, fd: i32, offset: u64) -> KResult<u64> {
    let file = p.fd.get(fd)?;
    let vp = match &file.kind {
        FileKind::Zero => return kern_mmap_anon(&p.vm(), at, len, prot),
        FileKind::Vnode(vp) => *vp,
        _ => return Err(Errno::ENODEV),
    };
    let attr = vp.getattr()?;
    if attr.kind != VType::Reg {
        return Err(Errno::ENODEV);
    }
    let flags = *file.flags.lock();
    if !flags.read || (shared && prot.write && !flags.write) {
        return Err(Errno::EACCES);
    }
    if !offset.is_multiple_of(u64_from_usize(PAGE_SIZE)) {
        return Err(Errno::EINVAL);
    }
    // The file's pages, from the page cache when touched; past its end,
    // zeros.
    let backing = crate::vnode_pager::vnode_backing(&vp, offset, attr.size.saturating_sub(offset))?;
    vm::vm_mmap::kern_mmap_backed(&p.vm(), at, len, prot, backing)
}
