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

//! `compat_freebsd`: the **FreeBSD 15.1 personality**.
//!
//! Nova runs FreeBSD binaries natively: this is its primary userland ABI.
//! Still, the ABI lives here, as a personality, exactly like FreeBSD keeps
//! its Linux ABI in `sys/compat/linux`. The MI kernel (`kern`, `vm`, ...)
//! knows nothing about FreeBSD syscall numbers, struct layouts, errno
//! values or sysctl names; it has its own internal interfaces, and this
//! crate translates. That keeps the kernel free to evolve, and leaves room
//! for other personalities (a native Nova ABI, Linux) next to this one.
//!
//! Layout (planned files in italics):
//!
//! | File                    | Role                                                     |
//! |-------------------------|----------------------------------------------------------|
//! | `freebsd_syscalls.rs`   | GENERATED from FreeBSD's `sys/kern/syscalls.master` by `tools/makesyscalls`: numbers and names |
//! | `freebsd_sysent.rs`     | the system call table: decodes arguments, calls the kernel |
//! | `freebsd_sysvec.rs`     | the `sysentvec`: brand matching, start-up, syscall entry |
//! | `freebsd_init.rs`       | the init policy: default `init_path`, `-s`              |
//! | `freebsd_exec.rs`       | the initial stack: strings, `ps_strings`, auxv           |
//! | `freebsd_errno.rs`      | kernel `Errno` → FreeBSD errno                           |
//! | `freebsd_signal.rs`     | kernel `Signal` → FreeBSD signal number                  |
//! | `freebsd_types.rs`      | `#[repr(C)]` structs (`stat`, `timespec`, `sigaction`...) with `const` layout checks |
//! | *`freebsd_sysctl.rs`*   | `kern.osreldate`, `hw.pagesize`, `kern.proc.*`...        |
//! | *`freebsd_ioctl.rs`*    | ioctl command translation                                |
//! | `arch/amd64/`           | the MD half: ELF machine (`elf_machdep.rs`), `USRSTACK` and start registers (`exec_machdep.rs`), syscall arguments (`trap.rs`) and results, error with the carry flag set (`vm_machdep.rs`); later the signal frame |
//!
//! The ELF format itself is not here: `sys/exec/elf` is the image
//! activator, shared by every ELF personality. This crate only recognizes
//! the FreeBSD brand and lays out the FreeBSD start-up state.
//!
//! `arch/` is the only place in this crate allowed to use
//! `cfg(target_arch)` (see docs/LAYERING.md, rule R3).

#![no_std]
#![forbid(unsafe_code)]

mod arch;
mod freebsd_errno;
mod freebsd_exec;
mod freebsd_kinfo;
mod freebsd_init;
mod freebsd_poll;
mod freebsd_proc;
mod freebsd_prot;
mod freebsd_resource;
mod freebsd_signal;
mod freebsd_syscalls;
mod freebsd_sysctl;
mod freebsd_sysent;
mod freebsd_sysvec;
mod freebsd_types;
mod freebsd_vfs;


pub use freebsd_init::FREEBSD_INIT;
pub use freebsd_sysvec::ELF64_FREEBSD_SYSVEC;

/// `__FreeBSD_version` of the ABI this personality implements
/// (`kern.osreldate`; FreeBSD 15.1-RELEASE).
pub const FREEBSD_VERSION: u32 = 1_501_000;
