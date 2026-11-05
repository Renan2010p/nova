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
//! vnodes): the file's pages are copied into anonymous memory when it is
//! mapped, so a private mapping is the file as it was, and a shared one
//! is the same on a read-only file system.
//!
//! TODO(vm): a page cache, pages read on demand and shared between
//! mappings, writable shared mappings.

use framework::PAGE_SIZE;
use framework::mm::{KVec, M_WAITOK};
use framework::user::Prot;
use libkern::conv::{u64_from_usize, usize_from_u64};
use sys::errno::{Errno, KResult};
use vm::vm_mmap::{MmapAt, kern_mmap_anon, vm_errno};

use crate::kern_descrip::FileKind;
use crate::kern_malloc::M_TEMP;
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
    let vm = p.vm();
    let addr = kern_mmap_anon(&vm, at, len, prot)?;
    // The file's bytes, a page at a time; past its end, zeros.
    let fill = || -> KResult<()> {
        let mut chunk: KVec<u8> = KVec::with_capacity(PAGE_SIZE, &M_TEMP, M_WAITOK)?;
        for _ in 0..PAGE_SIZE {
            chunk.push(0, M_WAITOK)?;
        }
        let avail = attr.size.saturating_sub(offset).min(len);
        let mut done = 0u64;
        while done < avail {
            let want = usize_from_u64((avail - done).min(u64_from_usize(PAGE_SIZE)));
            let buf = chunk.get_mut(..want).ok_or(Errno::EIO)?;
            let got = vp.read(offset + done, buf)?;
            if got == 0 {
                break;
            }
            vm.copyout_init(addr + done, buf.get(..got).ok_or(Errno::EIO)?).map_err(vm_errno)?;
            done += u64_from_usize(got);
        }
        Ok(())
    };
    if let Err(e) = fill() {
        let _ = vm.unmap(addr, len);
        return Err(e);
    }
    Ok(addr)
}
