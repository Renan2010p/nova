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

//! Executing an image (FreeBSD's `kern/kern_exec.c`).
//!
//! The core knows no executable format. It reads the first page of the
//! file and asks each registered [`ImageActivator`] whether it accepts
//! it. An activator (ELF is `sys/exec/elf`) loads the image into a fresh
//! address space, selects the ABI that runs it (`kern_abi::abi_select`),
//! and returns the registers to start with. An image no activator or ABI
//! accepts is `ENOEXEC`.
//!
//! The image comes from a file system ([`kern_execve_path`]) or from
//! memory (init, when the loader preloaded it); either way it is read
//! piece by piece ([`ImageFile`]), never copied whole.
//!
//! A new format is a new crate implementing [`ImageActivator`], registered
//! by `sys/conf`; the core does not change. TODO(exec): interpreters
//! (`PT_INTERP`, `#!`), set-id programs, stack growth, ASLR.

use libkern::conv::{u64_from_usize, usize_from_u64};
use framework::mm::{KArc, KVec, M_WAITOK};
use framework::sync::MtxSpin;
use framework::user::{self, UserRegs, VmSpace};
use sys::errno::{Errno, KResult};

use crate::kern_abi::SysEntVec;
use crate::kern_malloc::M_TEMP;
use crate::kern_proc::Proc;
use crate::vfs_syscalls::{AtFd, namei_at};
use crate::vnode::{VType, Vnode};

/// How much of the file the activators look at first: the header, the
/// program headers and the notes must be in it (FreeBSD reads one page,
/// `imgp->image_header`).
const IMAGE_HEADER: usize = 4096;

/// Most image activators a kernel is built with.
pub const MAX_ACTIVATORS: usize = 8;

/// The formats this kernel is built with, in registration order.
static ACTIVATORS: MtxSpin<[Option<&'static dyn ImageActivator>; MAX_ACTIVATORS]> =
    MtxSpin::new([None; MAX_ACTIVATORS]);

/// Registers an image format (`execsw`). Called by the kernel binary
/// before `mi_startup`.
///
/// # Panics
///
/// If [`MAX_ACTIVATORS`] formats are registered already: the kernel
/// binary is misconfigured.
pub fn register(act: &'static dyn ImageActivator) {
    let mut table = ACTIVATORS.lock();
    let Some(slot) = table.iter_mut().find(|s| s.is_none()) else {
        panic!("kern_exec: more than {MAX_ACTIVATORS} image activators");
    };
    *slot = Some(act);
}

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

/// What an activator gets besides the file: the name and strings of the
/// program (`execve`'s arguments).
#[derive(Clone, Copy, Debug)]
pub struct ExecArgs<'a> {
    /// The path executed (`AT_EXECPATH`).
    pub path: &'a str,
    /// Arguments, `argv[0]` first.
    pub argv: &'a [&'a str],
    /// Environment strings, `name=value`.
    pub envv: &'a [&'a str],
}

/// An executable format (FreeBSD's `execsw`). Implemented by services
/// such as `sys/exec/elf`; registered by `sys/conf`.
pub trait ImageActivator: Sync {
    /// Its name, for diagnostics (`"ELF64"`).
    fn name(&self) -> &'static str;

    /// Whether this format accepts the file whose first bytes are `head`.
    /// Must be cheap and must not allocate: it runs for every registered
    /// activator.
    fn probe(&self, head: &[u8]) -> bool;

    /// Loads the image into a fresh address space, selects the ABI and
    /// fills the registers the program starts with. `head` is the same
    /// prefix `probe` saw.
    fn exec(&self, head: &[u8], file: &dyn ImageFile, args: &ExecArgs<'_>) -> KResult<Image>;
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

/// Builds a new image from `file` (`kern_execve`): the first page is read
/// once, then the registered image activators are asked in order.
pub fn kern_execve(file: &dyn ImageFile, path: &str, argv: &[&str], envv: &[&str]) -> KResult<Image> {
    let head = image_head(file)?;

    let args = ExecArgs { path, argv, envv };
    // The lock is released before `exec`: the activator may allocate and
    // sleep, and must not run under a spin lock.
    let act = {
        let table = ACTIVATORS.lock();
        table.iter().flatten().copied().find(|a| a.probe(&head))
    };
    match act {
        Some(act) => act.exec(&head, file, &args),
        None => Err(Errno::ENOEXEC),
    }
}

/// Opens the file at the absolute `path` for an image activator: an
/// interpreter the image names (`PT_INTERP`, `#!`). It must be a regular
/// file.
pub fn image_open(path: &[u8]) -> KResult<Vnode> {
    let root = crate::vfs_mount::rootvnode().ok_or(Errno::ENOENT)?;
    let vp = crate::vfs_lookup::namei(root, root, path, true)?;
    if vp.getattr()?.kind != VType::Reg {
        return Err(Errno::EACCES);
    }
    Ok(vp)
}

/// Reads the first page of `file` (at most), as activators look at it.
pub fn image_head(file: &dyn ImageFile) -> KResult<KVec<u8>> {
    let head_len = usize_from_u64(file.size()?.min(u64_from_usize(IMAGE_HEADER)));
    let mut head: KVec<u8> = KVec::with_capacity(head_len, &M_TEMP, M_WAITOK)?;
    for _ in 0..head_len {
        head.push(0, M_WAITOK)?;
    }
    file.read_at(0, &mut head)?;
    Ok(head)
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
