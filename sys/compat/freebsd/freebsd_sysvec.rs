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

//! The FreeBSD `sysentvec` (`elf64_freebsd_sysvec`): what makes a
//! binary FreeBSD's, as `imgact_elf.c` decides it, and how it starts and
//! calls the kernel.

use framework::user::{UserFault, UserRegs, VmSpace};
use kern::kern_abi::{ExecImage, ImageParams, SysEntVec, SyscallOutcome};
use kern::kern_proc::Proc;
use sys::elf::notes;
use sys::errno::KResult;
use sys::signal::Signal;

use crate::arch::{
    ELF_ARCH, SV_NAME, USRSTACK, cpu_fetch_syscall_args, cpu_set_syscall_retval, exec_setregs,
    trap_uprintf_signal,
};
use crate::freebsd_sysent::{SyscallResult, syscall};
use crate::{freebsd_errno, freebsd_exec, freebsd_signal};

/// `ELFOSABI_FREEBSD`.
const ELFOSABI_FREEBSD: u8 = 9;
/// `NT_FREEBSD_ABI_TAG`, in a note owned by `FreeBSD`.
const NT_FREEBSD_ABI_TAG: u32 = 1;

/// The FreeBSD ABI.
pub struct FreebsdSysvec;

/// The instance the kernel binary registers.
pub static ELF64_FREEBSD_SYSVEC: FreebsdSysvec = FreebsdSysvec;

impl SysEntVec for FreebsdSysvec {
    fn name(&self) -> &'static str {
        SV_NAME
    }

    /// A FreeBSD binary for this machine: `EI_OSABI` is FreeBSD, or it
    /// carries a `FreeBSD` ABI tag note (`brandinfo`).
    fn accepts(&self, image: &ExecImage<'_>) -> bool {
        let Some(e) = image.ehdr else { return false };
        e.machine == ELF_ARCH
            && (e.osabi == ELFOSABI_FREEBSD
                || notes(image.header, &e)
                    .any(|n| n.name == b"FreeBSD" && n.kind == NT_FREEBSD_ABI_TAG))
    }

    fn usrstack(&self) -> u64 {
        USRSTACK
    }

    fn exec_setregs(&self, vm: &VmSpace, image: &ImageParams<'_>, regs: &mut UserRegs) -> KResult<()> {
        let stack = freebsd_exec::copyout_strings(vm, image)?;
        exec_setregs(regs, image.entry, stack);
        Ok(())
    }

    fn syscall(&self, p: &Proc, regs: &mut UserRegs) -> SyscallOutcome {
        let (code, args) = cpu_fetch_syscall_args(regs);
        match syscall(p, regs, code, &args) {
            SyscallResult::Ok(rv) => cpu_set_syscall_retval(regs, Ok(rv)),
            SyscallResult::Err(e) => cpu_set_syscall_retval(regs, Err(freebsd_errno::errno(e))),
            SyscallResult::Exit(rv) => return SyscallOutcome::Exit(rv),
            SyscallResult::Signal(sig) => return SyscallOutcome::Signal(sig),
        }
        SyscallOutcome::Return
    }

    fn trapsignal(&self, p: &Proc, fault: &UserFault, regs: &UserRegs) -> Signal {
        let sig = freebsd_signal::trapsignal(fault);
        if kern::init_main::bootverbose() {
            trap_uprintf_signal(p, regs, fault, freebsd_signal::signo(sig));
        }
        sig
    }

    fn signal_number(&self, sig: Signal) -> i32 {
        freebsd_signal::signo(sig)
    }
}
