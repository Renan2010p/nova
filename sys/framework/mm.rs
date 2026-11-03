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
//! * [`kmem`]: the kernel heap behind Rust's `alloc` (`Box`, `Vec`, ...),
//!   the role of FreeBSD's `malloc(9)` and UMA.
//!
//! Soundness rests on ownership: the allocator hands out each page once,
//! and frame contents are reached only through their owner, by copying
//! (`read_bytes`/`write_bytes`) rather than by handing out references, so
//! that a frame can later be mapped into user space without the kernel
//! holding Rust references into memory that userland can change.
//!
//! TODO(vm): `VmSpace` over the MD `pmap` (user address spaces), the
//! `vm_page` array, NUMA domains, and per-CPU page caches (smp).

pub mod frame;
pub mod kmem;
pub mod kstack;
pub mod vm_phys;

pub use frame::{Frame, Segment};
