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

//! Userland ABIs, as the kernel sees them: not at all.
//!
//! The kernel does not know any ABI. It knows only this interface,
//! FreeBSD's `struct sysentvec` (`sys/sysent.h`) as a trait. Every ABI
//! lives in `sys/compat/<abi>` (`freebsd` today) and implements
//! it; the kernel binary (`sys/conf`) registers the ones it is built
//! with. When a program is executed, the kernel asks each registered ABI
//! whether it accepts the image (FreeBSD's brand matching in
//! `imgact_elf.c`), and the process keeps the one that did.
//!
//! So the kernel never sees a syscall number, a userland struct layout, an
//! errno value or a signal frame: those are the ABI's business, behind
//! this trait. `checklayers` enforces the other half (rule R9): nothing but
//! `sys/conf` may depend on a `sys/compat` crate.
//!
//! TODO(exec): with user mode (milestone 4) the trait grows the rest of
//! `sysentvec`: setting up a new image (initial stack, auxv), dispatching
//! syscalls from a trap frame, delivering signals, translating errors.

use alloc::vec::Vec;

use framework::sync::MtxSpin;
use sys::elf::Ehdr;

/// The first bytes of a file being executed, as the image activator
/// reads them (`struct image_params`, `image_header`).
#[derive(Clone, Copy, Debug)]
pub struct ExecImage<'a> {
    /// The beginning of the file (at least the ELF and program headers).
    pub header: &'a [u8],
    /// The parsed ELF header, if the file is a 64-bit ELF.
    pub ehdr: Option<Ehdr>,
}

impl<'a> ExecImage<'a> {
    /// Wraps the first bytes of a file.
    pub fn new(header: &'a [u8]) -> Self {
        Self {
            header,
            ehdr: Ehdr::parse(header),
        }
    }
}

/// A userland ABI (FreeBSD's `struct sysentvec`). Implemented only in
/// `sys/compat/*`.
pub trait SysEntVec: Sync {
    /// The ABI's name (`sv_name`), e.g. `FreeBSD ELF64`.
    fn name(&self) -> &'static str;

    /// Whether this ABI runs `image` (`brandinfo` matching).
    fn accepts(&self, image: &ExecImage<'_>) -> bool;
}

/// The ABIs this kernel is built with, in the order they were registered.
static SYSVECS: MtxSpin<Vec<&'static dyn SysEntVec>> = MtxSpin::new(Vec::new());

/// Adds an ABI (`elf_insert_brand_entry`). Called by the kernel binary
/// before `mi_startup`.
pub fn abi_register(sv: &'static dyn SysEntVec) {
    SYSVECS.lock().push(sv);
}

/// The ABI that runs `image`: the first registered one that accepts it.
pub fn abi_select(image: &ExecImage<'_>) -> Option<&'static dyn SysEntVec> {
    SYSVECS.lock().iter().copied().find(|sv| sv.accepts(image))
}

/// The names of the registered ABIs.
pub fn abi_names() -> Vec<&'static str> {
    SYSVECS.lock().iter().map(|sv| sv.name()).collect()
}
