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

//! `freebsd_kpi`: the FreeBSD kernel programming interface (KPI), as safe
//! Rust wrappers over `framework`.
//!
//! A driver source ported from FreeBSD imports this crate instead of
//! FreeBSD's kernel headers. The wrappers are thin on purpose: they do not
//! reimplement the kernel, they expose the framework's safe mechanisms
//! under the names a FreeBSD driver expects. A bug in a driver that uses
//! only this crate cannot corrupt kernel memory.
//!
//! This is **not** a binary `.ko` loader. FreeBSD modules are built
//! against FreeBSD's internal KBI (symbols, struct layouts, `sysinit`,
//! `MODULE_DEPEND`, ...); Nova only implements the FreeBSD *syscall* ABI.
//! A port uses the FreeBSD source here, in Rust, behind this KPI.
//!
//! Implemented today:
//!
//! * [`malloc`]: `malloc(9)` — `MallocType`, `M_WAITOK`/`M_NOWAIT`, and
//!   the owning `KBox`/`KVec`/`KArc`;
//! * [`mtx`]: `mtx(9)` — the framework's spin mutex (`Mtx`, FreeBSD's
//!   `MTX_SPIN`) and sleep mutex (`SleepMtx`, `MTX_DEF`), with wait
//!   queues.
//!
//! Planned, in this order, as the framework grows (milestone 7):
//!
//! * `bus_space` — MMIO/PIO regions, width and barrier semantics;
//! * `bus_dma` — DMA tags, maps, segments and bounce buffers;
//! * `callout` — deferred work over the clock;
//! * `newbus` — `device_t`, `driver_t`, probe/attach/detach;
//! * `ifnet`, `bio` — the network and block interfaces.
//!
//! A driver crate lives under `sys/dev/freebsd/<name>` and depends on
//! this crate plus `framework`; it never depends on `sys/compat/*` or on
//! the FreeBSD userland personality.

#![no_std]
#![forbid(unsafe_code)]

pub mod malloc {
    //! `malloc(9)`: explicit allocation with a type and sleep flags.
    //!
    //! FreeBSD's `malloc(9)` and the owning containers Nova uses for it.
    //!
    //! Every allocation names a [`MallocType`] (accounting, as
    //! `vmstat -m`) and exactly one of `M_WAITOK` (may sleep) or
    //! `M_NOWAIT`; it returns a `Result`, never panics on exhaustion.

    pub use framework::mm::{KArc, KBox, KVec, M_NOWAIT, M_WAITOK, M_ZERO, MFlags, MallocType};
}

pub mod mtx {
    //! `mtx(9)`: FreeBSD's mutexes.
    //!
    //! `Mtx` is FreeBSD's `MTX_SPIN`: interrupts are disabled on this CPU
    //! while it is held. `SleepMtx` is `MTX_DEF`: waiters block instead of
    //! spinning. Both are generic over the data they protect, as the
    //! framework's locks are; a port that used a bare `struct mtx` and
    //! global variables should put that state in a `Mtx`/`SleepMtx<T>`.

    pub use framework::sync::{Mutex as SleepMtx, MtxSpin as Mtx, WaitQueue};
}

pub use malloc::{KArc, KBox, KVec, M_NOWAIT, M_WAITOK, M_ZERO, MFlags, MallocType};
pub use mtx::{Mtx, SleepMtx, WaitQueue};
