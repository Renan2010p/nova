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
//! What an ABI does, and what the kernel does for it:
//!
//! | The ABI (`sys/compat/<abi>`)                 | The kernel (`sys/kern`)                     |
//! |----------------------------------------------|---------------------------------------------|
//! | recognizes its images ([`SysEntVec::accepts`]) | loads `PT_LOAD` segments, maps the stack  |
//! | lays out argv, envp, auxv and the registers  | runs the program ([`crate::subr_trap`])     |
//! | decodes a system call, encodes its result    | implements it (`kern_write`, `exit1`, ...)  |
//! | numbers errors and signals                   | decides which error, and acts on signals    |
//! | says which signal a fault raises             | delivers it (for now: kills the process)    |

use framework::sync::MtxSpin;
use framework::user::{UserFault, UserRegs, VmSpace};
use sys::elf::Ehdr;
use sys::errno::KResult;
use sys::signal::Signal;

use crate::kern_proc::Proc;

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

/// A loaded image, for the ABI to start (FreeBSD's `image_params` after
/// the activator ran).
#[derive(Clone, Copy, Debug)]
pub struct ImageParams<'a> {
    /// Entry point (`e_entry`).
    pub entry: u64,
    /// User address of the program headers (`AT_PHDR`), 0 if not loaded.
    pub phdr: u64,
    /// Size of a program header (`AT_PHENT`).
    pub phent: u16,
    /// Number of program headers (`AT_PHNUM`).
    pub phnum: u16,
    /// The path executed (`AT_EXECPATH`).
    pub execpath: &'a str,
    /// Arguments, `argv[0]` first.
    pub argv: &'a [&'a str],
    /// Environment strings, `name=value`.
    pub envv: &'a [&'a str],
}

/// What follows a system call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyscallOutcome {
    /// Return to the program (with the result the ABI stored in its
    /// registers).
    Return,
    /// The program called `exit` with this status.
    Exit(i32),
    /// The call raises this signal (e.g. `SIGSYS` for a number the ABI
    /// does not have).
    Signal(Signal),
}

/// A userland ABI (FreeBSD's `struct sysentvec`). Implemented only in
/// `sys/compat/*`.
pub trait SysEntVec: Sync {
    /// The ABI's name (`sv_name`), e.g. `FreeBSD ELF64`.
    fn name(&self) -> &'static str;

    /// Whether this ABI runs `image` (`brandinfo` matching).
    fn accepts(&self, image: &ExecImage<'_>) -> bool;

    /// Top of the user stack (`sv_usrstack`). The kernel maps the stack
    /// below it.
    fn usrstack(&self) -> u64;

    /// Writes the new image's strings, argument vectors and auxiliary
    /// vector below `usrstack` in `vm`, and sets the registers the program
    /// starts with (`sv_copyout_strings` and `sv_setregs`).
    fn exec_setregs(&self, vm: &VmSpace, image: &ImageParams<'_>, regs: &mut UserRegs) -> KResult<()>;

    /// Handles the system call the program made with `regs`: decodes it,
    /// calls the kernel, and stores the result or error in `regs`
    /// (`sv_fetch_syscall_args`, `sv_table`, `sv_set_syscall_retval`).
    fn syscall(&self, p: &Proc, regs: &mut UserRegs) -> SyscallOutcome;

    /// The signal a user fault raises. ABIs differ (a protection fault is
    /// `SIGBUS` for FreeBSD, `SIGSEGV` elsewhere), so it is theirs to say
    /// (FreeBSD's MD `trap()` and `sv_transtrap`).
    /// `p` is the process and `regs` its registers at the fault, for the
    /// ABI's diagnostics (FreeBSD's `machdep.uprintf_signal`).
    fn trapsignal(&self, p: &Proc, fault: &UserFault, regs: &UserRegs) -> Signal;

    /// The ABI's number for `sig` (FreeBSD's signal numbers, `sv_sigtbl`).
    fn signal_number(&self, sig: Signal) -> i32;
}

/// Most ABIs a kernel can be built with (a fixed table, as FreeBSD's
/// `elf_brand_list[MAX_BRANDS]`).
pub const MAX_BRANDS: usize = 8;

/// The ABIs this kernel is built with, in the order they were registered.
static SYSVECS: MtxSpin<[Option<&'static dyn SysEntVec>; MAX_BRANDS]> = MtxSpin::new([None; MAX_BRANDS]);

/// Adds an ABI (`elf_insert_brand_entry`). Called by the kernel binary
/// before `mi_startup`.
///
/// # Panics
///
/// If [`MAX_BRANDS`] ABIs are registered already: the kernel binary is
/// misconfigured.
pub fn abi_register(sv: &'static dyn SysEntVec) {
    let mut table = SYSVECS.lock();
    let Some(slot) = table.iter_mut().find(|s| s.is_none()) else {
        panic!("abi_register: more than {MAX_BRANDS} ABIs");
    };
    *slot = Some(sv);
}

/// The ABI that runs `image`: the first registered one that accepts it.
pub fn abi_select(image: &ExecImage<'_>) -> Option<&'static dyn SysEntVec> {
    SYSVECS.lock().iter().flatten().copied().find(|sv| sv.accepts(image))
}
