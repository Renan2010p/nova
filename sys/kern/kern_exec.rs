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

//! Executing an image (FreeBSD's `kern/kern_exec.c` and the ELF image
//! activator, `kern/imgact_elf.c`).
//!
//! The kernel loads the `PT_LOAD` segments and maps the stack; the ABI that
//! accepts the image (see [`crate::kern_abi`]) lays out its arguments and
//! registers. An image no registered ABI accepts is `ENOEXEC`.
//!
//! TODO(exec): interpreters (`PT_INTERP`), `ET_DYN`, files from a file
//! system, stack growth, ASLR.


use libkern::conv::{u64_from_usize, usize_from_u64};
use framework::PAGE_SIZE;
use framework::mm::{KArc, KVec, M_WAITOK};
use framework::user::{Prot, UserRegs, VM_MAXUSER_ADDRESS, VmSpace};
use sys::elf::{ET_EXEC, PF_W, PF_X, PT_LOAD, phdrs};
use sys::errno::{Errno, KResult};

use crate::kern_abi::{self, ExecImage, ImageParams, SysEntVec};
use crate::kern_malloc::M_TEMP;

/// Size of the stack mapped for a new image (FreeBSD reserves `maxssiz`
/// and grows into it; Nova maps this much up front).
pub const SGROWSIZ_INITIAL: u64 = 1 << 20;

/// What `kern_execve` produced: an address space, the registers to start
/// with, and the ABI that runs them.
pub struct Image {
    /// The new address space.
    pub vm: KArc<VmSpace>,
    /// The initial registers.
    pub regs: UserRegs,
    /// The ABI that accepted the image.
    pub abi: &'static dyn SysEntVec,
}

pub use vm::vm_mmap::vm_errno;

/// Builds a new image from the ELF file `file` (`kern_execve`).
pub fn kern_execve(file: &[u8], path: &str, argv: &[&str], envv: &[&str]) -> KResult<Image> {
    let image = ExecImage::new(file);
    let ehdr = image.ehdr.ok_or(Errno::ENOEXEC)?;
    if ehdr.kind != ET_EXEC {
        return Err(Errno::ENOEXEC);
    }
    let abi = kern_abi::abi_select(&image).ok_or(Errno::ENOEXEC)?;
    let vm = VmSpace::new()?;

    // Pages of all segments, with their protections merged: two segments
    // may share a page.
    let page = u64_from_usize(PAGE_SIZE);
    let mut pages: KVec<(u64, Prot)> = KVec::new(&M_TEMP);
    let mut phdr = 0;
    for ph in phdrs(file, &ehdr).filter(|ph| ph.kind == PT_LOAD) {
        let end = ph.vaddr.checked_add(ph.memsz).filter(|&e| e <= VM_MAXUSER_ADDRESS);
        let file_end = ph.offset.checked_add(ph.filesz).filter(|&e| e <= u64_from_usize(file.len()));
        let (Some(end), Some(_)) = (end, file_end) else {
            return Err(Errno::ENOEXEC);
        };
        if ph.filesz > ph.memsz {
            return Err(Errno::ENOEXEC);
        }
        for va in (ph.vaddr & !(page - 1)..end).step_by(PAGE_SIZE) {
            let i = match pages.binary_search_by_key(&va, |&(v, _)| v) {
                Ok(i) => i,
                Err(i) => {
                    pages.insert(i, (va, Prot { read: true, write: false, exec: false }), M_WAITOK)?;
                    i
                }
            };
            if let Some((_, p)) = pages.get_mut(i) {
                p.write |= ph.flags & PF_W != 0;
                p.exec |= ph.flags & PF_X != 0;
            }
        }
        if (ph.offset..ph.offset + ph.filesz).contains(&ehdr.phoff) {
            phdr = ph.vaddr + (ehdr.phoff - ph.offset);
        }
    }
    if pages.is_empty() {
        return Err(Errno::ENOEXEC);
    }
    for &(va, prot) in pages.iter() {
        vm.map_zeroed(va, page, prot).map_err(vm_errno)?;
    }
    for ph in phdrs(file, &ehdr).filter(|ph| ph.kind == PT_LOAD) {
        let bytes = &file[usize_from_u64(ph.offset)..usize_from_u64(ph.offset + ph.filesz)];
        vm.copyout_init(ph.vaddr, bytes).map_err(vm_errno)?;
    }

    let top = abi.usrstack();
    vm.map_zeroed(top - SGROWSIZ_INITIAL, SGROWSIZ_INITIAL, Prot { read: true, write: true, exec: false })
        .map_err(vm_errno)?;
    let params = ImageParams {
        entry: ehdr.entry,
        phdr,
        phent: ehdr.phentsize,
        phnum: ehdr.phnum,
        execpath: path,
        argv,
        envv,
    };
    let mut regs = UserRegs::default();
    abi.exec_setregs(&vm, &params, &mut regs)?;
    Ok(Image { vm, regs, abi })
}
