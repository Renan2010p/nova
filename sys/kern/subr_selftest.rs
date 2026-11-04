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

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use libkern::conv::usize_from_u32;

use framework::mm::{KArc, KBox, KVec, M_WAITOK, M_ZERO, MallocType};
use framework::sync::{Mutex, WaitQueue};
use framework::task::{self, Task};

use crate::kern_environment::StaticEnv;
use crate::kern_kthread::kthread_add;
use crate::kern_synch::pause;
use crate::printf;

/// The self-tests' memory, so their accounting is checked on its own.
static M_SELFTEST: MallocType = MallocType::new("selftest");

/// Runs every non-fatal self-test.
pub fn run(verbose: bool, hz: u32) {
    kmem(verbose);
    kthreads(verbose);
    mutex(verbose);
    preemption(verbose, hz);
}

/// Waits for a thread to exit (there is no `kthread_join` in FreeBSD;
/// tests poll, as `kthread_shutdown` does).
fn join(t: &KArc<Task>) {
    while !t.has_exited() {
        pause("join", 1);
    }
}

fn spawn(name: &'static str, f: impl FnOnce() + Send + 'static) -> KArc<Task> {
    match kthread_add(name, f) {
        Ok(t) => t,
        Err(e) => panic!("selftest: cannot create thread {name}: {e:?}"),
    }
}

/// A self-test allocation; failing one is a failed test.
fn must<T>(r: Result<T, framework::mm::AllocError>, what: &str) -> T {
    match r {
        Ok(v) => v,
        Err(_) => panic!("selftest: no memory for {what}"),
    }
}

/// The heap: slab objects, a large block, growth, zeroing, and every byte
/// accounted to its malloc type and given back.
fn kmem(verbose: bool) {
    let before = framework::mm::vm_phys::free_count();
    {
        let mut v: KVec<u64> = KVec::new(&M_SELFTEST);
        for i in 1..=1000 {
            must(v.push(i, M_WAITOK), "KVec::push");
        }
        let big = must(KBox::new([0xa5u8; 64 * 1024], &M_SELFTEST, M_WAITOK), "a large block");
        let mut small: KVec<KBox<u32>> = must(KVec::with_capacity(256, &M_SELFTEST, M_WAITOK), "KVec");
        for i in 0..256 {
            must(small.push(must(KBox::new(i, &M_SELFTEST, M_WAITOK), "KBox"), M_WAITOK), "KVec::push");
        }
        let zeroed: KVec<u8> = {
            let mut z = must(KVec::with_capacity(4096, &M_SELFTEST, M_WAITOK | M_ZERO), "KVec");
            must(z.extend_from_slice(&[0; 4096], M_WAITOK), "KVec::extend_from_slice");
            z
        };
        let shared = must(KArc::new(7u64, &M_SELFTEST, M_WAITOK), "KArc");
        let again = shared.clone();
        assert_eq!(v.iter().sum::<u64>(), 500_500, "selftest: kmem: KVec");
        assert!(big.iter().all(|&b| b == 0xa5), "selftest: kmem: large block");
        assert!(small.iter().zip(0..).all(|(b, i)| **b == i), "selftest: kmem: slab");
        assert!(zeroed.iter().all(|&b| b == 0), "selftest: kmem: M_ZERO");
        assert!(KArc::ptr_eq(&shared, &again) && *again == 7, "selftest: kmem: KArc");
        assert!(M_SELFTEST.inuse() > 64 * 1024, "selftest: kmem: accounting");
    }
    assert_eq!(M_SELFTEST.inuse(), 0, "selftest: kmem: memory not given back");
    let kept = before.saturating_sub(framework::mm::vm_phys::free_count());
    if verbose {
        printf!(
            "selftest: kmem: KBox, KVec and KArc work; {} allocations, all freed ({} pages kept by the heap's slabs)\n",
            M_SELFTEST.calls(),
            kept
        );
    }
}

/// Two threads that take turns, through a wait queue.
fn kthreads(verbose: bool) {
    struct PingPong {
        turn: AtomicU32,
        wq: WaitQueue,
        log: Mutex<KVec<&'static str>>,
    }
    const ROUNDS: u32 = 3;
    let pp = must(
        KArc::new(
            PingPong {
                turn: AtomicU32::new(0),
                wq: WaitQueue::new(),
                log: Mutex::new(KVec::new(&M_SELFTEST)),
            },
            &M_SELFTEST,
            M_WAITOK,
        ),
        "the ping pong state",
    );
    let threads = [("ping", 0), ("pong", 1)].map(|(name, me)| {
        let pp = pp.clone();
        spawn(name, move || {
            for _ in 0..ROUNDS {
                pp.wq.wait_until(|| pp.turn.load(Ordering::Acquire) % 2 == me);
                must(pp.log.lock().push(name, M_WAITOK), "the log");
                pp.turn.fetch_add(1, Ordering::Release);
                pp.wq.wake_all();
            }
        })
    });
    threads.iter().for_each(join);
    let log = pp.log.lock();
    assert!(
        log.len() == 2 * usize::try_from(ROUNDS).expect("3 fits in usize") && log.chunks(2).all(|c| c == ["ping", "pong"]),
        "selftest: kthreads: wrong order {:?}",
        *log
    );
    if verbose {
        let tids = threads.each_ref().map(|t| t.tid());
        printf!("selftest: kthreads: tids {:?} took turns:", tids);
        for name in log.iter() {
            printf!(" {}", name);
        }
        printf!("\n");
    }
}

/// Threads contending for a sleep mutex.
fn mutex(verbose: bool) {
    const THREADS: usize = 3;
    const ITERATIONS: u64 = 500;
    let counter = must(KArc::new(Mutex::new(0u64), &M_SELFTEST, M_WAITOK), "the counter");
    let threads: [KArc<Task>; THREADS] = core::array::from_fn(|_| {
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
    });
    threads.iter().for_each(join);
    let total = *counter.lock();
    let expected = ITERATIONS * u64::try_from(THREADS).expect("3 fits in u64");
    assert_eq!(total, expected, "selftest: mutex: lost updates");
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
/// are more spinners than CPUs, and a spinner never gives its CPU up: if
/// every one of them gets to run, the clock took CPUs away from some.
fn preemption(verbose: bool, hz: u32) {
    struct Spin {
        stop: AtomicBool,
        /// How far each spinner got.
        spins: KVec<AtomicU64>,
    }
    let ncpus = framework::cpu::mp_ncpus();
    let nspinners = usize_from_u32(ncpus) + 1;
    let mut spins = must(KVec::with_capacity(nspinners, &M_SELFTEST, M_WAITOK), "the counters");
    for _ in 0..nspinners {
        must(spins.push(AtomicU64::new(0), M_WAITOK), "the counters");
    }
    let st = must(
        KArc::new(Spin { stop: AtomicBool::new(false), spins }, &M_SELFTEST, M_WAITOK),
        "the spinners' state",
    );
    let quantum = (hz / 10).max(1);
    let nivcsw = task::involuntary_switches();
    let mut spinners: KVec<KArc<Task>> = must(KVec::with_capacity(nspinners, &M_SELFTEST, M_WAITOK), "the spinner list");
    for i in 0..nspinners {
        let st = st.clone();
        let t = spawn("spinner", move || {
            let Some(mine) = st.spins.get(i) else { return };
            while !st.stop.load(Ordering::Acquire) {
                mine.fetch_add(1, Ordering::Relaxed);
                core::hint::spin_loop();
            }
        });
        must(spinners.push(t, M_WAITOK), "the spinner list");
    }
    // While this thread sleeps, the spinners have every CPU and one more
    // spinner than CPUs: that one runs only if the clock preempts another.
    let ticks = 5 * quantum;
    pause("preempt", ticks);
    st.stop.store(true, Ordering::Release);
    spinners.iter().for_each(join);
    let preempted = task::involuntary_switches() - nivcsw;
    let starved = st.spins.iter().filter(|s| s.load(Ordering::Relaxed) == 0).count();
    assert!(
        starved == 0,
        "selftest: preemption: {starved} of {nspinners} spinners never ran on {ncpus} CPU(s)"
    );
    assert!(preempted > 0, "selftest: preemption: every spinner ran, but no involuntary switch");
    if verbose {
        printf!(
            "selftest: preemption: {} busy threads shared {} CPU(s) for {} ticks through {} involuntary switch(es) (quantum {})\n",
            nspinners, ncpus, ticks, preempted, quantum
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
