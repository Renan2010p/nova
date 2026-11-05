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

//! `exec_elf`: the ELF image activator (FreeBSD's `imgact_elf.c`).
//!
//! It is the service that turns an ELF file into an address space, the
//! registers the program starts with, and the ABI that will run it. The
//! kernel core (`kern::kern_exec`) knows only bytes ([`ImageFile`]), the
//! registered [`ImageActivator`]s and the address space; the ELF format
//! lives here, registered by `sys/conf` next to the other activators.
//!
//! It is a service: 100% safe Rust, and it never sees a syscall number or
//! a userland struct. Selecting the ABI is
//! `kern::kern_abi::abi_select`, which asks each registered personality
//! whether it accepts the image; the activator only describes the image.
//!
//! A new format (a script, PE, Mach-O, ...) is a new crate implementing
//! [`ImageActivator`], registered from `sys/conf`. The core does not
//! change.

#![no_std]
#![forbid(unsafe_code)]

use framework::PAGE_SIZE;
use framework::mm::{KVec, M_WAITOK, MallocType};
use framework::user::{Prot, UserRegs, VM_MAXUSER_ADDRESS, VmSpace};
use kern::kern_abi::{self, AuxEntry, ExecImage, ImageParams};
use kern::kern_exec::{ExecArgs, Image, ImageActivator, ImageFile, SGROWSIZ_INITIAL, vm_errno};
use libkern::conv::{u64_from_usize, usize_from_u64};
use sys::elf::{AT_PHDR, AT_PHENT, AT_PHNUM, ET_EXEC, Ehdr, PF_W, PF_X, PT_LOAD, phdrs};
use sys::errno::{Errno, KResult};

/// Memory taken while loading the image (short-lived, as `M_TEMP`).
static M_EXEC: MallocType = MallocType::new("exec");

/// The ELF64 activator.
pub struct Elf64Activator;

/// The instance `sys/conf` registers.
pub static ELF64_ACTIVATOR: Elf64Activator = Elf64Activator;

impl ImageActivator for Elf64Activator {
    fn name(&self) -> &'static str {
        "ELF64"
    }

    fn probe(&self, head: &[u8]) -> bool {
        Ehdr::parse(head).is_some()
    }

    fn exec(&self, head: &[u8], file: &dyn ImageFile, args: &ExecArgs<'_>) -> KResult<Image> {
        load(head, file, args)
    }
}

/// Loads the ELF image in `head`/`file` (`kern_execve` for ELF).
fn load(head: &[u8], file: &dyn ImageFile, args: &ExecArgs<'_>) -> KResult<Image> {
    let size = file.size()?;
    let ehdr = Ehdr::parse(head).ok_or(Errno::ENOEXEC)?;
    if ehdr.kind != ET_EXEC {
        return Err(Errno::ENOEXEC);
    }
    let abi = kern_abi::abi_select(&ExecImage::new(head)).ok_or(Errno::ENOEXEC)?;
    let vm = VmSpace::new()?;

    // Pages of all segments, with their protections merged: two segments
    // may share a page.
    let page = u64_from_usize(PAGE_SIZE);
    let mut pages: KVec<(u64, Prot)> = KVec::new(&M_EXEC);
    let mut phdr = 0;
    for ph in phdrs(head, &ehdr).filter(|ph| ph.kind == PT_LOAD) {
        let end = ph.vaddr.checked_add(ph.memsz).filter(|&e| e <= VM_MAXUSER_ADDRESS);
        let file_end = ph.offset.checked_add(ph.filesz).filter(|&e| e <= size);
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

    // The segments' contents, a page at a time.
    let mut chunk: KVec<u8> = KVec::with_capacity(PAGE_SIZE, &M_EXEC, M_WAITOK)?;
    for _ in 0..PAGE_SIZE {
        chunk.push(0, M_WAITOK)?;
    }
    for ph in phdrs(head, &ehdr).filter(|ph| ph.kind == PT_LOAD) {
        let mut done = 0;
        while done < ph.filesz {
            let n = usize_from_u64((ph.filesz - done).min(page));
            let buf = chunk.get_mut(..n).ok_or(Errno::ENOEXEC)?;
            file.read_at(ph.offset + done, buf)?;
            vm.copyout_init(ph.vaddr + done, buf).map_err(vm_errno)?;
            done += u64_from_usize(n);
        }
    }

    let top = abi.usrstack();
    vm.map_zeroed(top - SGROWSIZ_INITIAL, SGROWSIZ_INITIAL, Prot { read: true, write: true, exec: false })
        .map_err(vm_errno)?;

    // What this format contributes to the ABI's start-up information (the
    // program headers, for FreeBSD's auxv). The core carries it opaquely.
    let aux = [
        AuxEntry { tag: AT_PHDR, value: phdr },
        AuxEntry { tag: AT_PHENT, value: u64::from(ehdr.phentsize) },
        AuxEntry { tag: AT_PHNUM, value: u64::from(ehdr.phnum) },
    ];
    let params = ImageParams {
        entry: ehdr.entry,
        execpath: args.path,
        argv: args.argv,
        envv: args.envv,
        aux: &aux,
    };
    let mut regs = UserRegs::default();
    abi.exec_setregs(&vm, &params, &mut regs)?;
    Ok(Image { vm, regs, abi })
}
