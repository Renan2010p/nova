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

//! `kern`: the machine-independent kernel core (FreeBSD's `sys/kern`).
//!
//! Files keep FreeBSD's names and prefixes:
//!
//! | Prefix    | Meaning                                  | Examples (now and planned)        |
//! |-----------|------------------------------------------|-----------------------------------|
//! | `init_`   | system start-up                          | `init_main`                       |
//! | `kern_`   | core facilities                          | `kern_clock`, `kern_kthread`, `kern_synch`, `kern_fork`, `kern_exec` |
//! | `subr_`   | support routines                         | `subr_param`, `subr_prf`, `subr_selftest` |
//! | `sys_`    | generic syscall back-ends                | `sys_generic`, `sys_pipe`         |
//! | `vfs_`    | virtual file system                      | `vfs_lookup`, `vfs_mount`         |
//! | `uipc_`   | sockets and IPC                          | `uipc_socket`                     |
//! | `sched_`  | schedulers (policy; the framework switches) | `sched_4bsd`, later `sched_ule` |
//!
//! This crate is a framekernel *service*: 100% safe Rust, no
//! `cfg(target_arch)`, no knowledge of any userland ABI. It reaches the
//! hardware only through `framework`, and will reach userland only
//! through the personalities in `sys/compat`. It must be correct on any
//! number of CPUs: shared state lives behind framework locks or in
//! `CpuLocal` variables, never in `static mut`.

#![no_std]
#![forbid(unsafe_code)]

pub mod init_main;
pub mod kern_abi;
pub mod kern_clock;
pub mod kern_descrip;
pub mod kern_cons;
pub mod kern_environment;
pub mod kern_exec;
pub mod kern_exit;
pub mod kern_fork;
pub mod kern_kthread;
pub mod kern_linker;
pub mod kern_malloc;
pub mod kern_mib;
pub mod kern_proc;
pub mod kern_random;
pub mod kern_prot;
pub mod kern_shutdown;
pub mod kern_sig;
pub mod kern_synch;
pub mod kern_thr;
pub mod sched_4bsd;
pub mod subr_param;
pub mod subr_prf;
pub mod subr_selftest;
pub mod subr_trap;
pub mod sys_generic;
pub mod tty;
pub mod vfs_lookup;
pub mod vfs_mount;
pub mod vfs_syscalls;
pub mod vnode;
pub mod vers;
