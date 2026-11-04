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
//! has saved its context on the way out, and it is never on a run queue
//! meanwhile, so no CPU can pick a thread whose context is not saved yet:
//!
//! * a thread that gives up its CPU while runnable (yield, preemption) is
//!   put back on the run queue by the thread switched to, in
//!   `finish_switch`, after its context is saved;
//! * a thread woken while it is still switching away (it blocked, and a
//!   waker on another CPU was quick) is enqueued by the waker only once it
//!   is off its CPU (`unpark` waits for that).
//!
//! The CPU that switches never waits for another CPU, so neither wait can
//! deadlock. (Waiting for `on_cpu` while switching, as earlier versions
//! did, deadlocks when two CPUs each pick the thread the other is leaving;
//! QEMU with 8 CPUs showed it.)
//! The per-CPU state (current, idle and previous thread) is in `CpuLocal`s.
//!
//! # Memory
//!
//! Creating a thread is the only allocation here, and it is explicit
//! ([`spawn`] fails with [`AllocError`]). Scheduling never allocates: run
//! and wait queues are [`TaskQueue`]s, linked through the threads
//! themselves, as FreeBSD's `TAILQ`s are.

use core::cell::{Cell, RefCell, UnsafeCell};
use core::fmt::Write as _;
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};

use mdi::Context;

use crate::cpu::{self, IntrGuard, curcpu, intr_guard};
use crate::mm::kstack::KernelStack;
use crate::mm::{AllocError, KArc, KFnOnce, M_WAITOK, MallocType};
use crate::pcpu::CpuLocal;
use crate::sync::{MtxSpin, OnceLock};
use crate::user::{self, VmSpace};
use crate::{MAXCPU, Md};

const RUNNING: u8 = 0;
const RUNNABLE: u8 = 1;
const BLOCKED: u8 = 2;
const EXITED: u8 = 3;

/// Threads and their bodies (FreeBSD's `THREAD` zone).
static M_THREAD: MallocType = MallocType::new("thread");

/// Longest thread name (`MAXCOMLEN`).
pub const MAXCOMLEN: usize = 19;

/// A thread name, truncated to [`MAXCOMLEN`] bytes (`td_name`).
#[derive(Clone, Copy)]
struct TaskName {
    buf: [u8; MAXCOMLEN],
    len: u8,
}

impl TaskName {
    const EMPTY: Self = Self { buf: [0; MAXCOMLEN], len: 0 };

    fn new(args: core::fmt::Arguments<'_>) -> Self {
        let mut n = Self::EMPTY;
        // A full buffer only truncates the name.
        let _ = n.write_fmt(args);
        n
    }

    fn as_str(&self) -> &str {
        let bytes = self.buf.get(..usize::from(self.len)).unwrap_or(&[]);
        // Only whole characters are ever appended.
        core::str::from_utf8(bytes).unwrap_or("?")
    }
}

impl core::fmt::Write for TaskName {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for c in s.chars() {
            let mut tmp = [0u8; 4];
            let enc = c.encode_utf8(&mut tmp).as_bytes();
            let at = usize::from(self.len);
            let Some(dst) = self.buf.get_mut(at..at + enc.len()) else {
                return Err(core::fmt::Error);
            };
            dst.copy_from_slice(enc);
            self.len = u8::try_from(at + enc.len()).map_err(|_| core::fmt::Error)?;
        }
        Ok(())
    }
}

/// First thread id, as in FreeBSD (`PID_MAX + 1`).
const TID_FIRST: u64 = 100_000;
static NEXT_TID: AtomicU64 = AtomicU64::new(TID_FIRST);

/// A kernel thread (the mechanism half of FreeBSD's `struct thread`).
pub struct Task {
    tid: u64,
    name: TaskName,
    pcb: UnsafeCell<<Md as Context>::Pcb>,
    /// The stack the thread runs on, owned here so it lives exactly as
    /// long as the thread can run. `None` for the boot thread, which runs
    /// on the boot stack.
    _kstack: Option<KernelStack>,
    entry: MtxSpin<Option<KFnOnce>>,
    state: AtomicU8,
    on_cpu: AtomicBool,
    idle: bool,
    /// The user address space the thread runs in, if any.
    vmspace: MtxSpin<Option<KArc<VmSpace>>>,
    /// The next thread in the [`TaskQueue`] this one is on (a thread is on
    /// at most one: a run queue or a wait queue).
    link: MtxSpin<Option<KArc<Task>>>,
    /// True while on a [`TaskQueue`].
    queued: AtomicBool,
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
    pub fn name(&self) -> &str {
        self.name.as_str()
    }

    /// True once the thread has exited.
    pub fn has_exited(&self) -> bool {
        self.state.load(Ordering::Acquire) == EXITED
    }

    fn new(name: TaskName, entry: Option<KFnOnce>, idle: bool) -> Result<Self, AllocError> {
        let kstack = KernelStack::new().ok_or(AllocError)?;
        let mut pcb = Md::PCB_INIT;
        // SAFETY: the stack top is 16-byte aligned and its pages are mapped
        // and writable; the stack lives in this task, which is only dropped
        // after it can no longer run; `task_entry` never returns.
        unsafe { Md::cpu_thread_init(&mut pcb, kstack.top(), task_entry) };
        Ok(Self {
            tid: NEXT_TID.fetch_add(1, Ordering::Relaxed),
            name,
            pcb: UnsafeCell::new(pcb),
            _kstack: Some(kstack),
            entry: MtxSpin::new(entry),
            state: AtomicU8::new(RUNNABLE),
            on_cpu: AtomicBool::new(false),
            idle,
            vmspace: MtxSpin::new(None),
            link: MtxSpin::new(None),
            queued: AtomicBool::new(false),
        })
    }

    pub(crate) fn mark_blocked(&self) {
        self.state.store(BLOCKED, Ordering::Release);
    }
}

/// A FIFO of threads, linked through the threads themselves, so adding
/// and removing never allocates (FreeBSD's `TAILQ` of threads). A thread
/// is on at most one queue at a time.
pub struct TaskQueue {
    head: Option<KArc<Task>>,
    tail: Option<KArc<Task>>,
    len: usize,
}

impl TaskQueue {
    /// An empty queue.
    pub const fn new() -> Self {
        Self { head: None, tail: None, len: 0 }
    }

    /// Threads on the queue.
    pub fn len(&self) -> usize {
        self.len
    }

    /// True if no thread is queued.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Appends `task`.
    ///
    /// # Panics
    ///
    /// If `task` is already on a queue.
    pub fn push_back(&mut self, task: KArc<Task>) {
        let was = task.queued.swap(true, Ordering::Relaxed);
        assert!(!was, "TaskQueue: thread {} is already queued", task.tid);
        match self.tail.replace(task.clone()) {
            Some(old) => *old.link.lock() = Some(task),
            None => self.head = Some(task),
        }
        self.len += 1;
    }

    /// Removes the first thread.
    pub fn pop_front(&mut self) -> Option<KArc<Task>> {
        let head = self.head.take()?;
        self.head = head.link.lock().take();
        if self.head.is_none() {
            self.tail = None;
        }
        self.len -= 1;
        head.queued.store(false, Ordering::Relaxed);
        Some(head)
    }
}

impl Default for TaskQueue {
    fn default() -> Self {
        Self::new()
    }
}

/// Scheduling policy: which runnable thread runs next. Implemented by a
/// service and injected once with [`inject_scheduler`]. Neither method may
/// allocate: both run with interrupts off (use a [`TaskQueue`]).
pub trait Scheduler: Sync {
    /// A thread became runnable (`sched_add`).
    fn enqueue(&self, task: KArc<Task>);
    /// The next thread to run on this CPU, if any (`sched_choose`).
    fn pick_next(&self) -> Option<KArc<Task>>;
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
    current: RefCell<Option<KArc<Task>>>,
    idle: RefCell<Option<KArc<Task>>>,
    /// The thread this CPU just switched away from, until `finish_switch`.
    prev: RefCell<Option<KArc<Task>>>,
    /// Whether `finish_switch` puts `prev` back on the run queue (it was
    /// runnable, not blocked or exited).
    prev_requeue: Cell<bool>,
    /// The user address space active on this CPU. Kernel threads run in
    /// whatever space is active (the kernel half is the same in all), so
    /// it changes only when a thread with another space runs. Holding it
    /// here keeps it alive while the CPU uses it.
    vmspace: RefCell<Option<KArc<VmSpace>>>,
}

static PCPU: CpuLocal<PerCpu> = CpuLocal::new(
    [const {
        PerCpu {
            current: RefCell::new(None),
            idle: RefCell::new(None),
            prev: RefCell::new(None),
            prev_requeue: Cell::new(false),
            vmspace: RefCell::new(None),
        }
    }; MAXCPU],
);
static NEED_RESCHED: CpuLocal<AtomicBool> =
    CpuLocal::new([const { AtomicBool::new(false) }; MAXCPU]);

/// Makes the running boot code thread 0 and creates the boot CPU's idle
/// thread. Called once by `framework::__init`, after the heap is up.
pub(crate) fn init() {
    let thread0 = Task {
        tid: NEXT_TID.fetch_add(1, Ordering::Relaxed),
        name: TaskName::new(format_args!("swapper")),
        pcb: UnsafeCell::new(Md::PCB_INIT),
        _kstack: None,
        entry: MtxSpin::new(None),
        state: AtomicU8::new(RUNNING),
        on_cpu: AtomicBool::new(true),
        idle: false,
        vmspace: MtxSpin::new(None),
        link: MtxSpin::new(None),
        queued: AtomicBool::new(false),
    };
    let thread0 =
        KArc::new(thread0, &M_THREAD, M_WAITOK).expect("task::init: no memory for thread 0");
    let idle = new_idle(0).expect("task::init: no memory for the idle thread");
    let g = intr_guard();
    let p = PCPU.get(&g);
    *p.current.borrow_mut() = Some(thread0);
    *p.idle.borrow_mut() = Some(idle);
}

/// Creates a kernel thread named `name` (truncated to [`MAXCOMLEN`])
/// running `f`, and makes it runnable (`kthread_add`). It allocates the
/// thread, its stack and its body, so it may sleep.
///
/// # Panics
///
/// If no scheduler has been injected, or from a context that may not
/// sleep.
pub fn spawn(name: &str, f: impl FnOnce() + Send + 'static) -> Result<KArc<Task>, AllocError> {
    let sched = SCHEDULER.get().expect("spawn: no scheduler");
    let body = KFnOnce::new(f, &M_THREAD, M_WAITOK)?;
    let task = Task::new(TaskName::new(format_args!("{name}")), Some(body), false)?;
    let task = KArc::new(task, &M_THREAD, M_WAITOK)?;
    sched.enqueue(task.clone());
    kick_idle_cpu();
    Ok(task)
}

/// The running thread (`curthread`).
pub fn current() -> KArc<Task> {
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
pub(crate) fn unpark(task: KArc<Task>) {
    let woken = task
        .state
        .compare_exchange(BLOCKED, RUNNABLE, Ordering::AcqRel, Ordering::Relaxed)
        .is_ok();
    if let (true, Some(sched)) = (woken, SCHEDULER.get()) {
        // It may still be switching away on another CPU (it blocked and
        // was woken at once): enqueue it only once its context is saved.
        // That CPU waits for nobody while switching, so this ends.
        while task.on_cpu.load(Ordering::Acquire) {
            cpu::cpu_spinwait();
        }
        sched.enqueue(task);
        kick_idle_cpu();
    }
}

/// Interrupt exit, with interrupts off and no guard held: switch if a
/// preemption was requested (FreeBSD's `ast` from `doreti`).
pub(crate) fn preempt_point() {
    if NEED_RESCHED.get_on(curcpu()).swap(false, Ordering::Relaxed) {
        let g = intr_guard();
        reschedule_why(&g, Switch::Involuntary);
    }
}

/// Why a thread switches away.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Switch {
    /// It yielded, blocked or exited (FreeBSD's `SW_VOL`).
    Voluntary,
    /// The clock preempted it (`SW_INVOL`).
    Involuntary,
}

/// Involuntary context switches so far, all threads, all CPUs (the sum of
/// FreeBSD's `ru_nivcsw`).
static NIVCSW: AtomicU64 = AtomicU64::new(0);

/// Involuntary context switches so far: how many times a running thread
/// was preempted and another one run instead.
pub fn involuntary_switches() -> u64 {
    NIVCSW.load(Ordering::Relaxed)
}

/// Picks the next thread and switches to it (`mi_switch`). The caller
/// holds `guard`, which must be the only interrupt guard alive.
pub(crate) fn reschedule(guard: &IntrGuard) {
    reschedule_why(guard, Switch::Voluntary);
}

fn reschedule_why(guard: &IntrGuard, why: Switch) {
    assert_eq!(
        cpu::intr_depth(),
        1,
        "mi_switch: switch with spin locks or critical sections held"
    );
    let Some(&sched) = SCHEDULER.get() else {
        return;
    };

    let (cur, next, requeue) = {
        let p = PCPU.get(guard);
        let cur = p
            .current
            .borrow()
            .clone()
            .expect("mi_switch: no current thread");
        if cur.idle && cur.state.load(Ordering::Relaxed) == BLOCKED {
            panic!("mi_switch: the idle thread blocked");
        }
        // Still runnable (yield, preemption): it goes back on the run
        // queue, but only once it is off this CPU (`finish_switch`).
        let requeue = !cur.idle
            && cur
                .state
                .compare_exchange(RUNNING, RUNNABLE, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok();
        let next = match sched.pick_next() {
            Some(next) => next,
            // Nothing else to run: a runnable thread just keeps its CPU.
            None if requeue => {
                cur.state.store(RUNNING, Ordering::Relaxed);
                return;
            }
            None => p.idle.borrow().clone().expect("mi_switch: no idle thread"),
        };
        (cur, next, requeue)
    };
    if KArc::ptr_eq(&cur, &next) {
        cur.state.store(RUNNING, Ordering::Relaxed);
        return;
    }
    if next.has_exited() {
        panic!("mi_switch: scheduler chose exited thread {}", next.tid);
    }
    // Run queues only hold threads that are off every CPU (see "SMP").
    assert!(
        !next.on_cpu.load(Ordering::Acquire),
        "mi_switch: thread {} chosen while still on a CPU",
        next.tid
    );
    next.on_cpu.store(true, Ordering::Relaxed);
    next.state.store(RUNNING, Ordering::Relaxed);
    activate_vmspace(guard, &next);
    let old = cur.pcb.get();
    let new = next.pcb.get().cast_const();
    {
        let p = PCPU.get(guard);
        // `prev` keeps the old thread (and its stack) alive until the new
        // one has finished the switch.
        *p.prev.borrow_mut() = Some(cur);
        p.prev_requeue.set(requeue);
        *p.current.borrow_mut() = Some(next);
    }
    if why == Switch::Involuntary {
        NIVCSW.fetch_add(1, Ordering::Relaxed);
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

/// Activates `task`'s address space on this CPU if it has one that is not
/// active already. Returns false if it has none. A space replaced here may
/// be freed here, with interrupts disabled.
fn activate_vmspace(guard: &IntrGuard, task: &Task) -> bool {
    let Some(vm) = task.vmspace.lock().clone() else {
        return false;
    };
    let mut active = PCPU.get(guard).vmspace.borrow_mut();
    if !active.as_ref().is_some_and(|a| KArc::ptr_eq(a, &vm)) {
        user::activate(Some(&vm));
        // The old space is no longer active here: release our reference.
        let old = active.replace(vm);
        drop(active);
        drop(old);
    }
    true
}

/// `user::set_vmspace`.
pub(crate) fn set_current_vmspace(vm: Option<KArc<VmSpace>>) {
    let cur = current();
    let old = core::mem::replace(&mut *cur.vmspace.lock(), vm);
    // If `old` is active on this CPU, the CPU's own reference keeps it
    // alive until another space replaces it; the kernel half it maps is
    // all a thread without a space uses.
    drop(old);
}

/// Activates the current thread's address space on this CPU; false if it
/// has none. For `user::enter`.
pub(crate) fn activate_current_vmspace(guard: &IntrGuard) -> bool {
    let cur = PCPU.get(guard).current.borrow().clone().expect("no current thread");
    activate_vmspace(guard, &cur)
}

/// The second half of every switch, run by the thread switched to: lets
/// the previous thread run elsewhere, puts it back on the run queue if it
/// is still runnable, and frees it if it exited.
fn finish_switch(guard: &IntrGuard) {
    // An AP's first switch (`ap_enter`) has no previous thread.
    let p = PCPU.get(guard);
    let Some(prev) = p.prev.borrow_mut().take() else {
        return;
    };
    let requeue = p.prev_requeue.replace(false);
    // Its context is saved: it may run anywhere from now on.
    prev.on_cpu.store(false, Ordering::Release);
    if requeue && let Some(&sched) = SCHEDULER.get() {
        sched.enqueue(prev);
        kick_idle_cpu();
        return;
    }
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
    entry.call();
    exit()
}

/// The idle thread: sleep until an interrupt, then see if anything became
/// runnable (`sched_idletd`).
fn idle_loop() -> ! {
    loop {
        IDLE.get_on(curcpu()).store(true, Ordering::Relaxed);
        cpu::cpu_idle();
        IDLE.get_on(curcpu()).store(false, Ordering::Relaxed);
        yield_now();
    }
}

/// CPUs sleeping in their idle thread, which an IPI can wake.
static IDLE: CpuLocal<AtomicBool> = CpuLocal::new([const { AtomicBool::new(false) }; MAXCPU]);

/// Something became runnable: wake one idle CPU other than this one, so
/// it does not wait for its next clock tick (FreeBSD's `sched_add` ->
/// `ipi_cpu(IPI_AST)`).
fn kick_idle_cpu() {
    let me = curcpu();
    // Claim the CPU (clear its flag) so that a second wakeup right after
    // this one goes to another idle CPU, not to the one already woken.
    if let Some(cpu) =
        crate::smp::online_cpus().find(|&c| c != me && IDLE.get_on(c).swap(false, Ordering::AcqRel))
    {
        crate::smp::ipi_cpu(cpu, mdi::Ipi::Ast);
    }
}

/// Creates the idle thread of CPU `cpu` (`idle: cpuN`).
pub(crate) fn new_idle(cpu: u32) -> Result<KArc<Task>, AllocError> {
    let task = Task::new(TaskName::new(format_args!("idle: cpu{cpu}")), None, true)?;
    KArc::new(task, &M_THREAD, M_WAITOK)
}

/// An AP's way into the scheduler: makes `idle` its current thread and
/// switches to it, abandoning the boot stack (FreeBSD's `sched_throw`).
pub(crate) fn ap_enter(idle: KArc<Task>) -> ! {
    // The switch invariant: exactly one guard. It lives on the boot stack,
    // which is never resumed; the idle thread takes over its count.
    let g = intr_guard();
    {
        let p = PCPU.get(&g);
        *p.idle.borrow_mut() = Some(idle.clone());
        *p.current.borrow_mut() = Some(idle.clone());
    }
    idle.on_cpu.store(true, Ordering::Relaxed);
    idle.state.store(RUNNING, Ordering::Relaxed);
    let new = idle.pcb.get().cast_const();
    drop(idle);
    let mut boot = Md::PCB_INIT;
    // SAFETY: interrupts are off; `boot` is a scratch PCB on this stack
    // that nothing will ever resume; `new` was set up by `cpu_thread_init`
    // and runs nowhere (the idle thread was just created for this CPU); the
    // idle thread is kept alive by this CPU's `PCPU.idle`.
    unsafe { Md::cpu_switch(&mut boot, new) };
    unreachable!("ap_enter: the boot stack was resumed");
}
