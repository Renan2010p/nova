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

//! Physical memory and the kernel heap: the framework's memory mechanisms.
//!
//! * [`vm_phys`]: the physical page allocator (FreeBSD's `vm/vm_phys.c`),
//!   a buddy allocator over `BootInfo::physavail`;
//! * [`Frame`] and [`Segment`]: owned physical pages. A page belongs to
//!   exactly one handle, and goes back to the allocator when it is dropped;
//! * [`malloc`]: explicit allocation, FreeBSD's `malloc(9)`: every
//!   allocation names its [`MallocType`] and flags and returns a `Result`;
//!   [`KBox`], [`KVec`], [`KArc`] and [`KFnOnce`] own what it returns. The
//!   kernel has no global allocator and does not use Rust's `alloc`;
//! * [`kmem`]: the heap under it (slabs and buddy blocks), FreeBSD's UMA.
//!
//! Soundness rests on ownership: the allocator hands out each page once,
//! and frame contents are reached only through their owner, by copying
//! (`read_bytes`/`write_bytes`) rather than by handing out references, so
//! that a frame can later be mapped into user space without the kernel
//! holding Rust references into memory that userland can change.
//!
//! User address spaces are in [`crate::user`].
//!
//! TODO(vm): the `vm_page` array, NUMA domains, and per-CPU page caches (smp).

pub mod frame;
pub(crate) mod kmem;
pub mod kstack;
pub mod malloc;
pub mod vm_phys;

pub use frame::{Frame, Segment};
pub use malloc::{AllocError, KArc, KBox, KFnOnce, KVec, M_NOWAIT, M_WAITOK, M_ZERO, MFlags, MallocType};
