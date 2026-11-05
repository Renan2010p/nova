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
//! The image comes from a file system ([`kern_execve_path`]) or from
//! memory (init, when the loader preloaded it); either way it is read
//! piece by piece ([`ImageFile`]), never copied whole.
//!
//! TODO(exec): interpreters (`PT_INTERP`), `ET_DYN`, `#!` scripts, set-id
//! programs, stack growth, ASLR.


use libkern::conv::{u64_from_usize, usize_from_u64};
use framework::PAGE_SIZE;
use framework::mm::{KArc, KVec, M_WAITOK};
use framework::user::{self, Prot, UserRegs, VM_MAXUSER_ADDRESS, VmSpace};
use sys::elf::{ET_EXEC, PF_W, PF_X, PT_LOAD, phdrs};
use sys::errno::{Errno, KResult};

use crate::kern_abi::{self, ExecImage, ImageParams, SysEntVec};
use crate::kern_malloc::M_TEMP;
use crate::kern_proc::Proc;
use crate::vfs_syscalls::{AtFd, namei_at};
use crate::vnode::{VType, Vnode};

/// How much of the file the image activator looks at first: the ELF
/// header, the program headers and the notes must be in it (FreeBSD reads
/// one page, `imgp->image_header`).
const IMAGE_HEADER: usize = 4096;

/// An executable file, read at offsets.
pub trait ImageFile {
    /// Its size.
    fn size(&self) -> KResult<u64>;
    /// Fills `buf` from `off`; `ENOEXEC` if the file ends first.
    fn read_at(&self, off: u64, buf: &mut [u8]) -> KResult<()>;
}

/// A file in memory (preloaded by the loader).
#[derive(Clone, Copy, Debug)]
pub struct MemFile<'a>(pub &'a [u8]);

impl ImageFile for MemFile<'_> {
    fn size(&self) -> KResult<u64> {
        Ok(u64_from_usize(self.0.len()))
    }

    fn read_at(&self, off: u64, buf: &mut [u8]) -> KResult<()> {
        let off = usize::try_from(off).map_err(|_| Errno::ENOEXEC)?;
        let src = self.0.get(off..off.checked_add(buf.len()).ok_or(Errno::ENOEXEC)?).ok_or(Errno::ENOEXEC)?;
        buf.copy_from_slice(src);
        Ok(())
    }
}

/// A file of a file system.
impl ImageFile for Vnode {
    fn size(&self) -> KResult<u64> {
        Ok(self.getattr()?.size)
    }

    fn read_at(&self, off: u64, buf: &mut [u8]) -> KResult<()> {
        let mut done = 0;
        while done < buf.len() {
            let n = self.read(off + u64_from_usize(done), buf.get_mut(done..).unwrap_or(&mut []))?;
            if n == 0 {
                return Err(Errno::ENOEXEC);
            }
            done += n;
        }
        Ok(())
    }
}

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
pub fn kern_execve(file: &dyn ImageFile, path: &str, argv: &[&str], envv: &[&str]) -> KResult<Image> {
    let size = file.size()?;
    let head_len = usize_from_u64(size.min(u64_from_usize(IMAGE_HEADER)));
    let mut header: KVec<u8> = KVec::with_capacity(head_len, &M_TEMP, M_WAITOK)?;
    for _ in 0..head_len {
        header.push(0, M_WAITOK)?;
    }
    file.read_at(0, &mut header)?;
    let image = ExecImage::new(&header);
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
    for ph in phdrs(&header, &ehdr).filter(|ph| ph.kind == PT_LOAD) {
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
    let mut chunk: KVec<u8> = KVec::with_capacity(PAGE_SIZE, &M_TEMP, M_WAITOK)?;
    for _ in 0..PAGE_SIZE {
        chunk.push(0, M_WAITOK)?;
    }
    for ph in phdrs(&header, &ehdr).filter(|ph| ph.kind == PT_LOAD) {
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

/// Executes the file at `path` in `p` (`kern_execve` from `execve(2)`):
/// on success `p` runs the new image, from the registers returned; on
/// failure it is unchanged.
pub fn kern_execve_path(p: &Proc, path: &[u8], argv: &[&str], envv: &[&str]) -> KResult<UserRegs> {
    let vp = namei_at(p, AtFd::Cwd, path, true)?;
    let attr = vp.getattr()?;
    if attr.kind != VType::Reg || attr.perm & 0o111 == 0 {
        return Err(Errno::EACCES);
    }
    let path = core::str::from_utf8(path).map_err(|_| Errno::EINVAL)?;
    let execpath = crate::kern_proc::path_vec(path.as_bytes())?;
    let image = kern_execve(&vp, path, argv, envv)?;
    Ok(exec_commit(p, path, execpath, image))
}

/// Makes `image` the image `p` runs (the end of `do_execve`): its address
/// space, ABI and name; descriptors marked close-on-exec are closed, and
/// caught signals go back to their default. Returns its registers.
pub fn exec_commit(p: &Proc, path: &str, execpath: KVec<u8>, image: Image) -> UserRegs {
    p.set_comm(path, execpath);
    p.set_abi(image.abi);
    user::set_vmspace(Some(image.vm.clone()));
    drop(p.set_vm(image.vm));
    p.fd.close_on_exec();
    p.sig.lock().exec();
    image.regs
}
