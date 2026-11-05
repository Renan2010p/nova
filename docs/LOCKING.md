# Locking in Nova

Nova runs on every CPU at once, and its services are safe Rust: the
compiler rules out data races (shared data is reachable only through a
lock, an atomic or a per-CPU variable; `Send` and `Sync` are checked;
there is no `static mut`, rule R6). What the compiler does not rule out is
**deadlock**: two paths taking the same locks in opposite orders. This
document gives the rules, and the order the kernel follows today.
`framework::sync::witness` checks it while the kernel runs.

## The locks

| Lock | Kind | Waiting | Where it may be taken |
|---|---|---|---|
| `MtxSpin` | spin mutex (`MTX_SPIN`) | spins, interrupts off on this CPU | anywhere, interrupt handlers included |
| `Mutex` | sleep mutex (`MTX_DEF`) | sleeps on a `WaitQueue` | thread context, no spin lock held |
| `WaitQueue` | sleep channel | sleeps until a condition holds | thread context, no spin lock held |
| `OnceLock` | set once, then read freely | none | anywhere |

## Rules

1. **No sleeping with a spin lock held.** `Mutex::lock`, `WaitQueue::wait_until`
   and every allocation with `M_WAITOK` check it at run time and panic
   (FreeBSD's "sleeping with a non-sleepable lock held"). So copying to or
   from user memory, which may allocate a page, is done after releasing
   spin locks (`msgbuf` and `vfs_mountroot` were fixed for this).
2. **A fixed order between lock classes.** A class is where a lock is
   created (`#[track_caller]` on `new`): all processes' signal locks are
   one class. If class A is ever held while B is taken, B is never held
   while A is taken, directly or through other locks. Locks of one class
   may nest (as FreeBSD's `MTX_DUPOK`).
3. **Interrupt context takes spin locks only**, and short: the terminal's
   input path records a signal for a kernel thread instead of signalling
   processes itself.
4. **Conditions read under a wait queue are atomics** (`wait_until`'s
   closure runs with the queue's spin lock held): `READY`, `EXIT_GEN`,
   a pipe's `count`, a process's `sig_hint`.
5. **Snapshot, then act.** Code walking the process table under its
   lock collects what it needs (`foreach_proc` filling a `KVec`,
   `kern_kill`), then copies out or sleeps after releasing it.
   Reading a directory collects the entries in a kernel buffer under
   the file system's lock and copies them out after (`getdirentries`).
6. **A page fault may read a file**: the address space's lock is held
   while the page cache asks the file system for a page, so a file
   system never touches user memory or address spaces under its locks.

## The order today

Sleep mutexes, outer first (an arrow: may be held while taking):

```
process table (ALLPROC) ──► a process's address space (VmSpace map)
a process's address space ──► page cache (vnode_pager) ──► a file system's nodes
open file's offset ──► a file system's nodes (tmpfs) ──► nothing
a socket's state ──► nothing (never two sockets at once: a send
    takes the peer's lock only, a connect drops the listener's first)
descriptor table ──► (closing a pipe end: only spin locks and wake-ups)
pipe ring ──► nothing
a process's exec path / arguments ──► nothing
```

Spin mutexes, outer first:

```
console (PRF_LOCK) ──► message buffer (MSGBUF) ──► console device
a process's signal state ──► (wake-ups: the wait queue's own lock)
process table walk (sleep) ──► a process's signal state, wchan, stop event
terminal (TTY) ──► nothing (signals go through the ttysig thread)
wait queue ──► scheduler run queues ──► task links
```

Sleep mutexes may be held while spin mutexes are taken (the other way is
rule 1), so no cycle crosses the two kinds: the checker follows each kind
on its own.

## The checker (`WITNESS`)

`framework::sync::witness` records, for every acquisition, which classes
the CPU (spin locks) or the thread (sleep mutexes) already holds, and
keeps the graph of orders seen. Taking B while holding A when B already
leads to A is a reversal; it is printed once per pair, as FreeBSD prints
it, with a backtrace:

```
lock order reversal:
 1st kern/subr_selftest.rs:309:28 (held)
 2nd kern/subr_selftest.rs:308:28 (being taken)
 2nd was taken before 1st elsewhere
```

It is lock-free and always on. A verbose boot prints how many classes and
orders it has seen (`selftest: smp: ... witness: N lock classes, M
orders, 0 reversals`); `debug.nova.selftest=witness` makes a reversal on
purpose, to check the checker.

## Tests of the primitives

* `selftest: mutex`: threads contending for a sleep mutex, yielding while
  holding it; no update may be lost;
* `selftest: smp`: producers and consumers on every CPU through a ring
  under a spin mutex, sleeping on wait queues when it is full or empty,
  with a sleep mutex hammered meanwhile: every item exactly once, no
  deadlock (the races of `wait_until` against `wake_one`/`wake_all`);
* `selftest: preemption`: more spinning threads than CPUs all get to run.

They run on every boot; `make qemu CPUS=4 VERBOSE=1` shows them.

## Not yet

* Processes have one thread: TLB shootdown (`framework/user.rs`) and
  user locks shared between threads (`_umtx_op`) come with `thr_new`.
* Shared/exclusive locks (`sx`, `rwlock`), and `WITNESS`'s lock names
  and `debug.witness` sysctls.
* Exhaustive checking of the primitives' interleavings (model checking,
  as Kani does for the sequential code; see VERIFICATION.md).
