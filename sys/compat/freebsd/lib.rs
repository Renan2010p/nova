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
//! Planned layout (milestone 4 onwards):
//!
//! | File                    | Role                                                     |
//! |-------------------------|----------------------------------------------------------|
//! | `freebsd_sysent.rs`     | GENERATED from FreeBSD's `sys/kern/syscalls.master` by `tools/makesyscalls` |
//! | `freebsd_sysvec.rs`     | the `sysentvec`: syscall table, errno translation, signal delivery, auxv |
//! | `freebsd_errno.rs`      | Nova errno → FreeBSD errno                               |
//! | `freebsd_types.rs`      | `#[repr(C)]` structs (`stat`, `timespec`, `sigaction`...) with `const` layout checks |
//! | `freebsd_sysctl.rs`     | `kern.osreldate`, `hw.pagesize`, `kern.proc.*`...        |
//! | `freebsd_ioctl.rs`      | ioctl command translation                                |
//! | `arch/amd64/`           | the MD half (`elf_machdep.rs`): ELF machine, later the syscall register convention (error in `%rax` with carry flag set), signal frame, initial stack |
//!
//! `arch/` is the only place in this crate allowed to use
//! `cfg(target_arch)` (see docs/LAYERING.md, rule R3).

#![no_std]
#![forbid(unsafe_code)]

mod arch;
mod freebsd_sysvec;

pub use freebsd_sysvec::ELF64_FREEBSD_SYSVEC;

/// `__FreeBSD_version` of the ABI this personality implements
/// (`kern.osreldate`; FreeBSD 15.1-RELEASE).
pub const FREEBSD_VERSION: u32 = 1_501_000;
