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

//! Boot-time self-tests of the kernel's own mechanisms: the heap, kernel
//! threads, sleep mutexes, preemption, and (on request) traps.
//!
//! They always run, and a failure panics. With `boot -v` each prints one
//! `selftest:` line. `debug.nova.selftest` in the loader asks for a fatal
//! test: `pf` (a kernel page fault) or `stack` (a kernel stack overflow,
//! caught by the guard page).

use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use framework::sync::{Mutex, WaitQueue};
use framework::task::{self, Task};

use crate::kern_environment::StaticEnv;
use crate::kern_kthread::kthread_add;
use crate::kern_synch::pause;
use crate::{kern_clock, printf};

/// Runs every non-fatal self-test.
pub fn run(verbose: bool, hz: u32) {
    kmem(verbose);
    kthreads(verbose);
    mutex(verbose);
    preemption(verbose, hz);
}

/// Waits for a thread to exit (there is no `kthread_join` in FreeBSD;
/// tests poll, as `kthread_shutdown` does).
fn join(t: &Arc<Task>) {
    while !t.has_exited() {
        pause("join", 1);
    }
}

fn spawn(name: &'static str, f: impl FnOnce() + Send + 'static) -> Arc<Task> {
    kthread_add(name, f).unwrap_or_else(|| panic!("selftest: cannot create thread {name}"))
}

/// The heap: slab objects, a large block, and freed memory coming back.
fn kmem(verbose: bool) {
    let before = framework::mm::vm_phys::free_count();
    {
        let v: Vec<u64> = (1..=1000).collect();
        let big: Box<[u8]> = alloc::vec![0xa5; 64 * 1024].into_boxed_slice();
        let small: Vec<Box<u32>> = (0..256).map(Box::new).collect();
        assert_eq!(v.iter().sum::<u64>(), 500_500, "selftest: kmem: Vec");
        assert!(
            big.iter().all(|&b| b == 0xa5),
            "selftest: kmem: large block"
        );
        assert!(
            small.iter().zip(0..).all(|(b, i)| **b == i),
            "selftest: kmem: slab"
        );
    }
    let kept = before.saturating_sub(framework::mm::vm_phys::free_count());
    if verbose {
        printf!(
            "selftest: kmem: Box and Vec work ({} pages kept by the heap's slabs)\n",
            kept
        );
    }
}

/// Two threads that take turns, through a wait queue.
fn kthreads(verbose: bool) {
    struct PingPong {
        turn: AtomicU32,
        wq: WaitQueue,
        log: Mutex<Vec<&'static str>>,
    }
    const ROUNDS: u32 = 3;
    let pp = Arc::new(PingPong {
        turn: AtomicU32::new(0),
        wq: WaitQueue::new(),
        log: Mutex::new(Vec::new()),
    });
    let threads: Vec<Arc<Task>> = [("ping", 0), ("pong", 1)]
        .into_iter()
        .map(|(name, me)| {
            let pp = pp.clone();
            spawn(name, move || {
                for _ in 0..ROUNDS {
                    pp.wq
                        .wait_until(|| pp.turn.load(Ordering::Acquire) % 2 == me);
                    pp.log.lock().push(name);
                    pp.turn.fetch_add(1, Ordering::Release);
                    pp.wq.wake_all();
                }
            })
        })
        .collect();
    threads.iter().for_each(join);
    let log = pp.log.lock();
    assert!(
        log.len() == 2 * ROUNDS as usize && log.chunks(2).all(|c| c == ["ping", "pong"]),
        "selftest: kthreads: wrong order {:?}",
        *log
    );
    if verbose {
        let tids: Vec<u64> = threads.iter().map(|t| t.tid()).collect();
        printf!(
            "selftest: kthreads: tids {:?} took turns: {}\n",
            tids,
            log.join(" ")
        );
    }
}

/// Threads contending for a sleep mutex.
fn mutex(verbose: bool) {
    const THREADS: u64 = 3;
    const ITERATIONS: u64 = 500;
    let counter = Arc::new(Mutex::new(0u64));
    let threads: Vec<Arc<Task>> = (0..THREADS)
        .map(|_| {
            let counter = counter.clone();
            spawn("mtxtest", move || {
                for i in 0..ITERATIONS {
                    let mut c = counter.lock();
                    *c += 1;
                    if i % 50 == 0 {
                        // Yield holding the mutex: the others must block.
                        task::yield_now();
                    }
                }
            })
        })
        .collect();
    threads.iter().for_each(join);
    let total = *counter.lock();
    assert_eq!(total, THREADS * ITERATIONS, "selftest: mutex: lost updates");
    if verbose {
        printf!(
            "selftest: mutex: {} threads x {} increments = {}\n",
            THREADS,
            ITERATIONS,
            total
        );
    }
}

/// Threads that never yield must still be preempted by the clock. There
/// is one spinner per CPU, so no CPU is left for this thread unless the
/// clock takes one away from a spinner.
fn preemption(verbose: bool, hz: u32) {
    let stop = Arc::new(AtomicBool::new(false));
    let spins = Arc::new(AtomicU64::new(0));
    let running = Arc::new(AtomicU32::new(0));
    let ncpus = framework::cpu::mp_ncpus();
    let spinners: Vec<Arc<Task>> = (0..ncpus)
        .map(|_| {
            let (stop, spins, running) = (stop.clone(), spins.clone(), running.clone());
            spawn("spinner", move || {
                running.fetch_add(1, Ordering::AcqRel);
                while !stop.load(Ordering::Acquire) {
                    spins.fetch_add(1, Ordering::Relaxed);
                    core::hint::spin_loop();
                }
            })
        })
        .collect();
    // Wait until the spinners hold every other CPU, so that none is idle
    // when this thread wakes up.
    while running.load(Ordering::Acquire) < ncpus - 1 {
        task::yield_now();
    }
    let start = kern_clock::ticks();
    // While this thread sleeps the last spinner takes this CPU too. Waking
    // up only lets us run again if the clock preempts one of them.
    pause("preempt", 1);
    let back = kern_clock::ticks();
    stop.store(true, Ordering::Release);
    spinners.iter().for_each(join);
    assert!(
        spins.load(Ordering::Relaxed) > 0,
        "selftest: preemption: the spinner never ran"
    );
    if verbose {
        printf!(
            "selftest: preemption: {} busy thread(s) on {} CPU(s), one preempted after {} ticks (quantum {})\n",
            spinners.len(),
            framework::cpu::mp_ncpus(),
            back - start,
            (hz / 10).max(1)
        );
    }
}

/// `debug.nova.selftest=pf` or `=stack`: a fatal trap on purpose.
pub fn trap_selftest(kenv: &StaticEnv<'_>) {
    match kenv.getenv("debug.nova.selftest") {
        Some("pf") => {
            printf!("selftest: taking a page fault on purpose\n");
            framework::cpu::cpu_selftest_fault();
        }
        Some("stack") => {
            printf!("selftest: overflowing a kernel stack on purpose\n");
            spawn("overflow", || {
                core::hint::black_box(recurse(0));
            });
            loop {
                pause("overflow", 100);
            }
        }
        _ => {}
    }
}

/// Unbounded recursion that the compiler cannot turn into a loop.
#[inline(never)]
fn recurse(depth: u64) -> u64 {
    let frame = core::hint::black_box([depth; 32]);
    if frame[0] == u64::MAX {
        return 0;
    }
    recurse(depth + 1).wrapping_add(frame[1])
}
