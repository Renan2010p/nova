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
//!
//! Executables (`ET_EXEC`) load where they are linked; position-
//! independent ones (`ET_DYN`) where their ABI says (`pie_base`). An
//! image naming an interpreter (`PT_INTERP`, the dynamic linker) gets it
//! loaded too, where `mmap` puts things (`mmap_base`); the program starts
//! in the interpreter, which finds the program through `AT_BASE`,
//! `AT_ENTRY` and `AT_PHDR`.
//!
//! TODO(exec): ASLR, `PT_GNU_STACK`, segments mapped from the file
//! (shared, copy-on-write) instead of copied.

#![no_std]
#![forbid(unsafe_code)]

use framework::PAGE_SIZE;
use framework::mm::{KVec, M_WAITOK, MallocType};
use framework::user::{Prot, UserRegs, VM_MAXUSER_ADDRESS, VmSpace};
use kern::kern_abi::{self, AuxEntry, ExecImage, ImageParams};
use framework::user::MapPlace;
use kern::kern_exec::{ExecArgs, Image, ImageActivator, ImageFile, SGROWSIZ_INITIAL, image_head, image_open, vm_errno};
use libkern::conv::{u64_from_usize, usize_from_u64};
use sys::elf::{
    AT_BASE, AT_ENTRY, AT_PHDR, AT_PHENT, AT_PHNUM, ET_DYN, ET_EXEC, Ehdr, PF_W, PF_X, PT_INTERP, PT_LOAD, phdrs,
};
use sys::errno::{Errno, KResult};

/// Longest interpreter path.
const INTERP_MAX: usize = 1024;

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

/// What loading an object's segments gave.
struct Loaded {
    /// User address of its program headers, if a segment holds them.
    phdr: u64,
    /// Its entry point, relocated.
    entry: u64,
}

/// Maps the `PT_LOAD` segments of `ehdr` at `bias` plus their addresses,
/// with their protections merged where they share a page, and fills them
/// from `file` (`__elfN(load_sections)`).
fn load_segments(vm: &VmSpace, head: &[u8], ehdr: &Ehdr, file: &dyn ImageFile, bias: u64) -> KResult<Loaded> {
    let size = file.size()?;
    let page = u64_from_usize(PAGE_SIZE);
    let mut pages: KVec<(u64, Prot)> = KVec::new(&M_EXEC);
    let mut phdr = 0;
    for ph in phdrs(head, ehdr).filter(|ph| ph.kind == PT_LOAD) {
        let start = ph.vaddr.checked_add(bias).ok_or(Errno::ENOEXEC)?;
        let end = start.checked_add(ph.memsz).filter(|&e| e <= VM_MAXUSER_ADDRESS);
        let file_end = ph.offset.checked_add(ph.filesz).filter(|&e| e <= size);
        let (Some(end), Some(_)) = (end, file_end) else {
            return Err(Errno::ENOEXEC);
        };
        if ph.filesz > ph.memsz {
            return Err(Errno::ENOEXEC);
        }
        for va in (start & !(page - 1)..end).step_by(PAGE_SIZE) {
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
            phdr = start + (ehdr.phoff - ph.offset);
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
    for ph in phdrs(head, ehdr).filter(|ph| ph.kind == PT_LOAD) {
        let mut done = 0;
        while done < ph.filesz {
            let n = usize_from_u64((ph.filesz - done).min(page));
            let buf = chunk.get_mut(..n).ok_or(Errno::ENOEXEC)?;
            file.read_at(ph.offset + done, buf)?;
            vm.copyout_init(ph.vaddr + bias + done, buf).map_err(vm_errno)?;
            done += u64_from_usize(n);
        }
    }
    Ok(Loaded { phdr, entry: ehdr.entry.wrapping_add(bias) })
}

/// The address range `[low, high)` the `PT_LOAD` segments span, in pages.
fn span(head: &[u8], ehdr: &Ehdr) -> KResult<(u64, u64)> {
    let page = u64_from_usize(PAGE_SIZE);
    let mut low = u64::MAX;
    let mut high = 0;
    for ph in phdrs(head, ehdr).filter(|ph| ph.kind == PT_LOAD) {
        low = low.min(ph.vaddr & !(page - 1));
        high = high.max(ph.vaddr.checked_add(ph.memsz).ok_or(Errno::ENOEXEC)?);
    }
    if low >= high {
        return Err(Errno::ENOEXEC);
    }
    Ok((low, high.next_multiple_of(page)))
}

/// Loads the interpreter at `path` (`__elfN(load_file)`) where `mmap`
/// would put it; returns its base (`AT_BASE`) and entry point.
fn load_interp(vm: &VmSpace, path: &[u8], base_hint: u64) -> KResult<(u64, u64)> {
    let vp = image_open(path)?;
    let head = image_head(&vp)?;
    let ehdr = Ehdr::parse(&head).ok_or(Errno::ENOEXEC)?;
    if ehdr.kind != ET_DYN {
        return Err(Errno::ENOEXEC);
    }
    let (low, high) = span(&head, &ehdr)?;
    // A free range, found as `mmap` would find one, then released for the
    // segments.
    let none = Prot { read: false, write: false, exec: false };
    let base = vm.map_anon(base_hint, high - low, none, MapPlace::Hint { align_shift: 21 }).map_err(vm_errno)?;
    vm.unmap(base, high - low).map_err(vm_errno)?;
    let loaded = load_segments(vm, &head, &ehdr, &vp, base - low)?;
    Ok((base, loaded.entry))
}

/// The interpreter path an image names, if any (`PT_INTERP`).
fn interp_path<'a>(head: &[u8], ehdr: &Ehdr, file: &dyn ImageFile, buf: &'a mut [u8; INTERP_MAX]) -> KResult<Option<&'a [u8]>> {
    let Some(ph) = phdrs(head, ehdr).find(|ph| ph.kind == PT_INTERP) else { return Ok(None) };
    let len = usize_from_u64(ph.filesz);
    if len == 0 || len > INTERP_MAX {
        return Err(Errno::ENOEXEC);
    }
    let b = buf.get_mut(..len).ok_or(Errno::ENOEXEC)?;
    file.read_at(ph.offset, b)?;
    // NUL-terminated in the file.
    let end = b.iter().position(|&c| c == 0).unwrap_or(len);
    Ok(Some(buf.get(..end).unwrap_or(&[])))
}

/// Loads the ELF image in `head`/`file` (`kern_execve` for ELF).
fn load(head: &[u8], file: &dyn ImageFile, args: &ExecArgs<'_>) -> KResult<Image> {
    let ehdr = Ehdr::parse(head).ok_or(Errno::ENOEXEC)?;
    let abi = kern_abi::abi_select(&ExecImage::new(head)).ok_or(Errno::ENOEXEC)?;
    let bias = match ehdr.kind {
        ET_EXEC => 0,
        ET_DYN => abi.pie_base() - span(head, &ehdr)?.0,
        _ => return Err(Errno::ENOEXEC),
    };
    let vm = VmSpace::new()?;
    let prog = load_segments(&vm, head, &ehdr, file, bias)?;

    let mut ibuf = [0u8; INTERP_MAX];
    let interp = match interp_path(head, &ehdr, file, &mut ibuf)? {
        Some(path) => Some(load_interp(&vm, path, abi.mmap_base())?),
        None => None,
    };

    let top = abi.usrstack();
    vm.map_zeroed(top - SGROWSIZ_INITIAL, SGROWSIZ_INITIAL, Prot { read: true, write: true, exec: false })
        .map_err(vm_errno)?;

    // What this format contributes to the ABI's start-up information (the
    // program headers, where the interpreter is, where the program
    // starts). The core carries it opaquely.
    let aux = [
        AuxEntry { tag: AT_PHDR, value: prog.phdr },
        AuxEntry { tag: AT_PHENT, value: u64::from(ehdr.phentsize) },
        AuxEntry { tag: AT_PHNUM, value: u64::from(ehdr.phnum) },
        AuxEntry { tag: AT_BASE, value: interp.map_or(0, |(base, _)| base) },
        AuxEntry { tag: AT_ENTRY, value: prog.entry },
    ];
    let params = ImageParams {
        // The interpreter runs first, if there is one.
        entry: interp.map_or(prog.entry, |(_, entry)| entry),
        execpath: args.path,
        argv: args.argv,
        envv: args.envv,
        aux: &aux,
    };
    let mut regs = UserRegs::default();
    abi.exec_setregs(&vm, &params, &mut regs)?;
    Ok(Image { vm, regs, abi })
}
