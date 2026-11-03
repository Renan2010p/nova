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

//! Kernel threads: the mechanism (FreeBSD's `mi_switch`, `sched_switch`,
//! `fork_exit`, `thread_exit`), without the policy.
//!
//! The framework owns what must be trusted: thread stacks, saved contexts,
//! the switch itself, and the rules that keep them sound. *Which* thread
//! runs next is decided by a [`Scheduler`] that a service injects
//! (`kern/sched_4bsd.rs`), like Asterinas' OSTD does, so scheduling policy
//! is 100% safe code.
//!
//! # The switch invariant
//!
//! Every switch happens with **exactly one** interrupt guard held: the
//! caller's (`yield_now`, `exit`, a `WaitQueue`, or the preemption point at
//! interrupt exit). The thread switched to resumes with one guard held too
//! (its own, from when it switched away), or, if it is new, takes over the
//! switcher's count in [`task_entry`]. So the per-CPU guard count is right
//! on every CPU at all times, and no thread can switch away while holding a
//! spin lock (that is a panic, like FreeBSD's "switch with spin locks held").
//!
//! # SMP
//!
//! A thread is `on_cpu` from the moment a CPU chooses it until that CPU
//! has saved its context on the way out. Another CPU that picks it in
//! between waits, so a context is never loaded before it is fully saved.
//! The per-CPU state (current, idle and previous thread) is in `CpuLocal`s.

use alloc::boxed::Box;
use alloc::sync::Arc;
use core::cell::{RefCell, UnsafeCell};
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};

use mdi::Context;

use crate::cpu::{self, IntrGuard, curcpu, intr_guard};
use crate::mm::kstack::KernelStack;
use crate::pcpu::CpuLocal;
use crate::sync::{MtxSpin, OnceLock};
use crate::{MAXCPU, Md};

const RUNNING: u8 = 0;
const RUNNABLE: u8 = 1;
const BLOCKED: u8 = 2;
const EXITED: u8 = 3;

/// First thread id, as in FreeBSD (`PID_MAX + 1`).
const TID_FIRST: u64 = 100_000;
static NEXT_TID: AtomicU64 = AtomicU64::new(TID_FIRST);

/// A kernel thread (the mechanism half of FreeBSD's `struct thread`).
pub struct Task {
    tid: u64,
    name: &'static str,
    pcb: UnsafeCell<<Md as Context>::Pcb>,
    /// The stack the thread runs on, owned here so it lives exactly as
    /// long as the thread can run. `None` for the boot thread, which runs
    /// on the boot stack.
    _kstack: Option<KernelStack>,
    entry: MtxSpin<Option<Box<dyn FnOnce() + Send>>>,
    state: AtomicU8,
    on_cpu: AtomicBool,
    idle: bool,
}

// SAFETY: every field but `pcb` is Sync. `pcb` is written only by
// `cpu_switch` on the CPU the task is leaving, and read only by a CPU
// switching to it after `on_cpu` was released (Release) and observed false
// (Acquire); the two never overlap.
unsafe impl Sync for Task {}

impl Task {
    /// Thread id (`td_tid`).
    pub fn tid(&self) -> u64 {
        self.tid
    }

    /// Thread name (`td_name`).
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// True once the thread has exited.
    pub fn has_exited(&self) -> bool {
        self.state.load(Ordering::Acquire) == EXITED
    }

    fn new(
        name: &'static str,
        entry: Option<Box<dyn FnOnce() + Send>>,
        idle: bool,
    ) -> Option<Self> {
        let kstack = KernelStack::new()?;
        let mut pcb = Md::PCB_INIT;
        // SAFETY: the stack top is 16-byte aligned and its pages are mapped
        // and writable; the stack lives in this task, which is only dropped
        // after it can no longer run; `task_entry` never returns.
        unsafe { Md::cpu_thread_init(&mut pcb, kstack.top(), task_entry) };
        Some(Self {
            tid: NEXT_TID.fetch_add(1, Ordering::Relaxed),
            name,
            pcb: UnsafeCell::new(pcb),
            _kstack: Some(kstack),
            entry: MtxSpin::new(entry),
            state: AtomicU8::new(RUNNABLE),
            on_cpu: AtomicBool::new(false),
            idle,
        })
    }

    pub(crate) fn mark_blocked(&self) {
        self.state.store(BLOCKED, Ordering::Release);
    }
}

/// Scheduling policy: which runnable thread runs next. Implemented by a
/// service and injected once with [`inject_scheduler`].
pub trait Scheduler: Sync {
    /// A thread became runnable (`sched_add`).
    fn enqueue(&self, task: Arc<Task>);
    /// The next thread to run on this CPU, if any (`sched_choose`).
    fn pick_next(&self) -> Option<Arc<Task>>;
}

static SCHEDULER: OnceLock<&'static dyn Scheduler> = OnceLock::new();

/// Installs the scheduling policy. Before this, threads can be created
/// but nothing switches.
///
/// # Panics
///
/// If a scheduler is already installed.
pub fn inject_scheduler(sched: &'static dyn Scheduler) {
    assert!(
        SCHEDULER.set(sched).is_ok(),
        "inject_scheduler: scheduler already installed"
    );
}

struct PerCpu {
    current: RefCell<Option<Arc<Task>>>,
    idle: RefCell<Option<Arc<Task>>>,
    /// The thread this CPU just switched away from, until `finish_switch`.
    prev: RefCell<Option<Arc<Task>>>,
}

static PCPU: CpuLocal<PerCpu> = CpuLocal::new(
    [const {
        PerCpu {
            current: RefCell::new(None),
            idle: RefCell::new(None),
            prev: RefCell::new(None),
        }
    }; MAXCPU],
);
static NEED_RESCHED: CpuLocal<AtomicBool> =
    CpuLocal::new([const { AtomicBool::new(false) }; MAXCPU]);

/// Makes the running boot code thread 0 and creates the boot CPU's idle
/// thread. Called once by `framework::__init`, after the heap is up.
pub(crate) fn init() {
    let thread0 = Arc::new(Task {
        tid: NEXT_TID.fetch_add(1, Ordering::Relaxed),
        name: "swapper",
        pcb: UnsafeCell::new(Md::PCB_INIT),
        _kstack: None,
        entry: MtxSpin::new(None),
        state: AtomicU8::new(RUNNING),
        on_cpu: AtomicBool::new(true),
        idle: false,
    });
    let idle =
        Task::new("idle: cpu0", None, true).expect("task::init: no memory for the idle thread");
    let g = intr_guard();
    let p = PCPU.get(&g);
    *p.current.borrow_mut() = Some(thread0);
    *p.idle.borrow_mut() = Some(Arc::new(idle));
}

/// Creates a kernel thread running `f` and makes it runnable
/// (`kthread_add`). `None` if memory is exhausted.
///
/// # Panics
///
/// If no scheduler has been injected.
pub fn spawn(name: &'static str, f: impl FnOnce() + Send + 'static) -> Option<Arc<Task>> {
    let sched = SCHEDULER.get().expect("spawn: no scheduler");
    let task = Arc::new(Task::new(name, Some(Box::new(f)), false)?);
    sched.enqueue(task.clone());
    Some(task)
}

/// The running thread (`curthread`).
pub fn current() -> Arc<Task> {
    let g = intr_guard();
    PCPU.get(&g)
        .current
        .borrow()
        .clone()
        .expect("current: no thread")
}

/// Lets other runnable threads run (`kern_yield`).
///
/// # Panics
///
/// If an interrupt guard or spin lock is held.
pub fn yield_now() {
    cpu::assert_no_guard("yield with a spin lock or critical section held");
    let g = intr_guard();
    reschedule(&g);
}

/// Ends the current thread (`kthread_exit`). Its stack is freed once
/// another thread runs.
pub fn exit() -> ! {
    cpu::assert_no_guard("thread exiting with a spin lock or critical section held");
    let g = intr_guard();
    current().state.store(EXITED, Ordering::Release);
    reschedule(&g);
    unreachable!("task::exit: an exited thread was resumed");
}

/// Asks this CPU to switch threads at the next interrupt exit (sets
/// `TDF_NEEDRESCHED`). Safe to call from interrupt context.
pub fn request_preempt() {
    NEED_RESCHED.get_on(curcpu()).store(true, Ordering::Relaxed);
}

/// Makes a blocked thread runnable again (`setrunnable`).
pub(crate) fn unpark(task: Arc<Task>) {
    let woken = task
        .state
        .compare_exchange(BLOCKED, RUNNABLE, Ordering::AcqRel, Ordering::Relaxed)
        .is_ok();
    if let (true, Some(sched)) = (woken, SCHEDULER.get()) {
        sched.enqueue(task);
    }
}

/// Interrupt exit, with interrupts off and no guard held: switch if a
/// preemption was requested (FreeBSD's `ast` from `doreti`).
pub(crate) fn preempt_point() {
    if NEED_RESCHED.get_on(curcpu()).swap(false, Ordering::Relaxed) {
        let g = intr_guard();
        reschedule(&g);
    }
}

/// Picks the next thread and switches to it (`mi_switch`). The caller
/// holds `guard`, which must be the only interrupt guard alive.
pub(crate) fn reschedule(guard: &IntrGuard) {
    assert_eq!(
        cpu::intr_depth(),
        1,
        "mi_switch: switch with spin locks or critical sections held"
    );
    let Some(&sched) = SCHEDULER.get() else {
        return;
    };

    let (cur, next) = {
        let p = PCPU.get(guard);
        let cur = p
            .current
            .borrow()
            .clone()
            .expect("mi_switch: no current thread");
        if cur.idle && cur.state.load(Ordering::Relaxed) == BLOCKED {
            panic!("mi_switch: the idle thread blocked");
        }
        if !cur.idle
            && cur
                .state
                .compare_exchange(RUNNING, RUNNABLE, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
        {
            sched.enqueue(cur.clone());
        }
        let next = sched
            .pick_next()
            .unwrap_or_else(|| p.idle.borrow().clone().expect("mi_switch: no idle thread"));
        (cur, next)
    };
    if Arc::ptr_eq(&cur, &next) {
        cur.state.store(RUNNING, Ordering::Relaxed);
        return;
    }
    if next.has_exited() {
        panic!("mi_switch: scheduler chose exited thread {}", next.tid);
    }

    // SMP: wait until the CPU that last ran `next` has saved its context.
    while next.on_cpu.load(Ordering::Acquire) {
        cpu::cpu_spinwait();
    }
    next.on_cpu.store(true, Ordering::Relaxed);
    next.state.store(RUNNING, Ordering::Relaxed);
    let old = cur.pcb.get();
    let new = next.pcb.get() as *const _;
    {
        let p = PCPU.get(guard);
        // `prev` keeps the old thread (and its stack) alive until the new
        // one has finished the switch.
        *p.prev.borrow_mut() = Some(cur);
        *p.current.borrow_mut() = Some(next);
    }
    // SAFETY: interrupts are off (we hold a guard). `old` is the running
    // thread's PCB, kept alive by `prev` until it is resumed and by its
    // owner after; `new` was set up by `cpu_thread_init` or saved by an
    // earlier switch, and `on_cpu` guarantees no CPU is running or saving
    // it. No reference into either PCB exists.
    unsafe { Md::cpu_switch(old, new) };

    // We are `cur` again, maybe on another CPU: per-CPU state is read anew.
    finish_switch(guard);
}

/// The second half of every switch, run by the thread switched to: lets
/// the previous thread run elsewhere, and frees it if it exited.
fn finish_switch(guard: &IntrGuard) {
    let prev = PCPU
        .get(guard)
        .prev
        .borrow_mut()
        .take()
        .expect("finish_switch: no previous thread");
    prev.on_cpu.store(false, Ordering::Release);
    // Dropping the last reference to an exited thread frees its stack;
    // we are on another stack.
    drop(prev);
}

/// Where every new thread starts, from its first `cpu_switch`.
extern "C" fn task_entry() -> ! {
    {
        let g = intr_guard();
        finish_switch(&g);
    }
    // The guard that switched to us lives on the previous thread's stack;
    // this thread takes over its count and turns interrupts on.
    cpu::switch_entry_release();

    let current = current();
    if current.idle {
        drop(current);
        idle_loop();
    }
    let entry = current
        .entry
        .lock()
        .take()
        .expect("task_entry: thread started twice");
    drop(current);
    entry();
    exit()
}

/// The idle thread: sleep until an interrupt, then see if anything became
/// runnable (`sched_idletd`).
fn idle_loop() -> ! {
    loop {
        cpu::cpu_idle();
        yield_now();
    }
}
