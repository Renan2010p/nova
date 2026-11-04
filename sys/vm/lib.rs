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

//! `vm`: the machine-independent virtual memory system (FreeBSD's
//! `sys/vm`): the policy over the framework's `VmSpace`, which owns the
//! mechanisms (pages, page tables) because that is where soundness is
//! decided. Today: [`vm_mmap`] (anonymous mappings) and [`vm_fault`]
//! (demand-zero paging).
//!
//! Planned files, with their FreeBSD names:
//!
//! | File          | Role                                                     |
//! |---------------|----------------------------------------------------------|
//! | `vm_page`     | `struct vm_page`, one per physical page                  |
//! | `vm_object`   | objects backing mappings (anonymous, vnode, device)      |
//! | `vm_map`      | address spaces: maps of entries over objects             |
//! | `vm_fault`    | page fault resolution, copy-on-write                     |
//! | `vm_pageout`  | page reclamation                                         |
//! | `uma`         | slab allocator policy (zones, caches) per CPU            |
//!
//! The split is the classic BSD one, made strict by the framekernel: this
//! crate (a safe service) decides *what* is mapped where; the framework
//! owns the mechanisms (`Frame` handles from the physical allocator,
//! `VmSpace` over the MD `pmap`) and guarantees that safe code can never
//! map a frame it does not own. Dependency cycles with VFS (the vnode
//! pager) are broken with a `Pager` trait defined here and implemented in
//! `kern`.

#![no_std]
#![forbid(unsafe_code)]

pub mod vm_fault;
pub mod vm_mmap;
