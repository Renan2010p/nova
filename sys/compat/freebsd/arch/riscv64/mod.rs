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

//! FreeBSD/riscv64 (FreeBSD's `riscv/riscv/elf_machdep.c`).

mod elf_machdep;
mod exec_machdep;
mod sig_machdep;
mod sys_machdep;
mod trap;
mod vm_machdep;

pub(crate) use elf_machdep::*;
pub(crate) use exec_machdep::{PAGESIZES, USRSTACK, exec_setregs};
pub(crate) use sig_machdep::{cpu_syscall_restart, map_sigcode, sendsig, sigreturn};
pub(crate) use sys_machdep::sysarch;
pub(crate) use trap::{cpu_fetch_syscall_args, trap_uprintf_signal};
pub(crate) use vm_machdep::*;
