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

//! The 4BSD scheduler (FreeBSD's `kern/sched_4bsd.c`), reduced to its
//! core: one run queue, round robin, and a time slice of `hz / 10` ticks
//! (`sched_quantum`), after which the running thread is preempted.
//!
//! This is the *policy*; the framework does the switching. It is safe
//! code, injected with `framework::task::inject_scheduler`.
//!
//! TODO(sched): priorities and their decay (`schedcpu`), per-CPU run
//! queues and CPU affinity (smp); then ULE (`sched_ule.c`), FreeBSD's
//! default.

use core::cell::Cell;
use core::sync::atomic::{AtomicU32, Ordering};

use framework::MAXCPU;
use framework::pcpu::CpuLocal;
use framework::sync::MtxSpin;
use framework::mm::KArc;
use framework::task::{self, Scheduler, Task, TaskQueue};

struct Sched4bsd {
    /// Runnable threads, linked through themselves: no allocation here.
    runq: MtxSpin<TaskQueue>,
}

impl Scheduler for Sched4bsd {
    fn enqueue(&self, task: KArc<Task>) {
        self.runq.lock().push_back(task);
    }

    fn pick_next(&self) -> Option<KArc<Task>> {
        let next = self.runq.lock().pop_front();
        if next.is_some() {
            // The chosen thread runs on this CPU with a fresh slice, as
            // 4BSD's sched_switch resets ts_slice.
            let g = framework::cpu::intr_guard();
            SLICE.get(&g).set(0);
        }
        next
    }
}

static SCHED: Sched4bsd = Sched4bsd {
    runq: MtxSpin::new(TaskQueue::new()),
};

/// Ticks a thread may run before being preempted (`sched_quantum`).
static SCHED_QUANTUM: AtomicU32 = AtomicU32::new(1);
/// Ticks the running thread has used of its slice, per CPU.
static SLICE: CpuLocal<Cell<u32>> = CpuLocal::new([const { Cell::new(0) }; MAXCPU]);

/// Installs the scheduler (`sched_setup`).
pub fn sched_init(hz: u32) {
    SCHED_QUANTUM.store((hz / 10).max(1), Ordering::Relaxed);
    task::inject_scheduler(&SCHED);
}

/// The scheduler's share of a clock tick (`sched_clock`): preempt the
/// running thread when its slice is used up.
pub fn sched_clock() {
    let g = framework::cpu::intr_guard();
    let slice = SLICE.get(&g);
    slice.set(slice.get() + 1);
    if slice.get() >= SCHED_QUANTUM.load(Ordering::Relaxed) {
        slice.set(0);
        task::request_preempt();
    }
}
