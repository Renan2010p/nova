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


//! Resource limits, priorities and interval timers (FreeBSD's
//! `kern/kern_resource.c` and the `itimer` half of `kern/kern_time.c`).
//!
//! Limits are kept per process and inherited; the kernel enforces the
//! number of open files (its own table's size bounds it anyway) and
//! nothing else yet. Priorities are kept for `getpriority`; the 4BSD
//! scheduler does not weigh them yet.
//!
//! TODO(resource): enforce the limits (`RLIMIT_DATA`, `RLIMIT_STACK`,
//! `RLIMIT_CPU`, ...), `ITIMER_VIRTUAL`/`ITIMER_PROF`, `getrusage`.

use core::sync::atomic::Ordering;

use framework::sync::WaitQueue;
use sys::errno::{Errno, KResult};
use sys::signal::Signal;

use crate::kern_clock::{hz, ticks};
use crate::kern_proc::{Proc, foreach_proc};

/// A resource a process may be limited in (`RLIMIT_*`, by name).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resource {
    /// CPU time, in seconds.
    Cpu,
    /// Largest file.
    FileSize,
    /// Data segment.
    Data,
    /// Stack.
    Stack,
    /// Core file.
    Core,
    /// Resident set.
    Rss,
    /// Locked memory.
    MemLock,
    /// Processes per user.
    NProc,
    /// Open files.
    NoFile,
    /// Socket buffers.
    SbSize,
    /// Address space.
    VMem,
    /// Pseudo-terminals.
    Npts,
    /// Swap.
    Swap,
    /// Kqueues.
    Kqueues,
    /// Process-shared user locks.
    Umtxp,
    /// Pipe buffers.
    PipeBuf,
}

/// How many resources there are.
pub const NRESOURCES: usize = 16;

impl Resource {
    /// Its slot in a process's table.
    pub const fn index(self) -> usize {
        match self {
            Resource::Cpu => 0,
            Resource::FileSize => 1,
            Resource::Data => 2,
            Resource::Stack => 3,
            Resource::Core => 4,
            Resource::Rss => 5,
            Resource::MemLock => 6,
            Resource::NProc => 7,
            Resource::NoFile => 8,
            Resource::SbSize => 9,
            Resource::VMem => 10,
            Resource::Npts => 11,
            Resource::Swap => 12,
            Resource::Kqueues => 13,
            Resource::Umtxp => 14,
            Resource::PipeBuf => 15,
        }
    }
}

/// No limit (`RLIM_INFINITY`, by meaning).
pub const RLIM_INFINITY: u64 = u64::MAX;

/// A limit: soft and hard (`struct rlimit`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rlimit {
    /// The limit in force.
    pub cur: u64,
    /// The most `cur` may be raised to.
    pub max: u64,
}

const INF: Rlimit = Rlimit { cur: RLIM_INFINITY, max: RLIM_INFINITY };

/// The limits a process starts with (FreeBSD amd64's defaults: `maxdsiz`
/// 32 GiB, `maxssiz` 512 MiB, files as the descriptor table allows).
pub const DEFAULT_LIMITS: [Rlimit; NRESOURCES] = {
    let mut l = [INF; NRESOURCES];
    l[Resource::Data.index()] = Rlimit { cur: 32 << 30, max: 32 << 30 };
    l[Resource::Stack.index()] = Rlimit { cur: 512 << 20, max: 512 << 20 };
    let files = libkern::conv::u64_from_usize(crate::kern_descrip::MAXFILES);
    l[Resource::NoFile.index()] = Rlimit { cur: files, max: files };
    l[Resource::MemLock.index()] = Rlimit { cur: 64 << 10, max: 64 << 10 };
    l
};

/// A limit of `p` (`kern_getrlimit`).
pub fn kern_getrlimit(p: &Proc, r: Resource) -> Rlimit {
    p.rlimits.lock().get(r.index()).copied().unwrap_or(INF)
}

/// Sets a limit of `p` (`kern_setrlimit`): the soft limit may not exceed
/// the hard one; as root, the hard one may be raised. The number of open
/// files stays within the descriptor table.
pub fn kern_setrlimit(p: &Proc, r: Resource, mut lim: Rlimit) -> KResult<()> {
    if lim.cur > lim.max {
        return Err(Errno::EINVAL);
    }
    if r == Resource::NoFile {
        let files = u64::try_from(crate::kern_descrip::MAXFILES).unwrap_or(u64::MAX);
        lim.cur = lim.cur.min(files);
        lim.max = lim.max.min(files);
    }
    let mut t = p.rlimits.lock();
    let slot = t.get_mut(r.index()).ok_or(Errno::EINVAL)?;
    *slot = lim;
    Ok(())
}

/// Whose priority `getpriority`/`setpriority` mean.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrioWho {
    /// A process (0: the caller).
    Process(u32),
    /// A process group (0: the caller's).
    Group(u32),
    /// A user's processes (0: the caller's).
    User(u32),
}

fn prio_targets(p: &Proc, who: PrioWho, mut f: impl FnMut(&Proc)) -> KResult<()> {
    let mut n = 0;
    foreach_proc(|q| {
        let hit = match who {
            PrioWho::Process(0) => q.pid == p.pid,
            PrioWho::Process(pid) => q.pid == pid,
            PrioWho::Group(0) => q.pgid() == p.pgid(),
            PrioWho::Group(g) => q.pgid() == g,
            PrioWho::User(0) => q.uid() == p.uid(),
            PrioWho::User(u) => q.uid() == u,
        };
        if hit && q.exit_status().is_none() {
            n += 1;
            f(q);
        }
    });
    if n > 0 { Ok(()) } else { Err(Errno::ESRCH) }
}

/// The highest priority (lowest nice value) among `who` (`getpriority`).
pub fn kern_getpriority(p: &Proc, who: PrioWho) -> KResult<i32> {
    let mut best = i32::MAX;
    prio_targets(p, who, |q| best = best.min(q.nice.load(Ordering::Relaxed)))?;
    Ok(best)
}

/// Sets the nice value of `who`, clamped to `-20..=20` (`setpriority`).
pub fn kern_setpriority(p: &Proc, who: PrioWho, nice: i32) -> KResult<()> {
    let nice = nice.clamp(-20, 20);
    prio_targets(p, who, |q| q.nice.store(nice, Ordering::Relaxed))
}

/// A real-time interval timer (`ITIMER_REAL`): when it fires, in clock
/// ticks, and its reload period (0: once).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Itimer {
    /// The tick it fires at; 0: disarmed.
    pub deadline: u64,
    /// Ticks between firings after the first; 0: none.
    pub interval: u64,
}

fn ns_to_ticks(ns: u64) -> u64 {
    let hz = u64::from(hz().max(1));
    u64::try_from((u128::from(ns) * u128::from(hz)).div_ceil(1_000_000_000)).unwrap_or(u64::MAX)
}

fn ticks_to_ns(t: u64) -> u64 {
    t.saturating_mul(1_000_000_000 / u64::from(hz().max(1)))
}

/// Arms (`value_ns` > 0) or disarms `p`'s real timer, reloading every
/// `interval_ns` (`kern_setitimer` with `ITIMER_REAL`); returns what was
/// left of the old one, and its interval, in nanoseconds.
pub fn kern_setitimer_real(p: &Proc, value_ns: u64, interval_ns: u64) -> (u64, u64) {
    let now = ticks();
    let mut t = p.itimer.lock();
    let old = *t;
    *t = if value_ns == 0 {
        Itimer::default()
    } else {
        Itimer { deadline: now + ns_to_ticks(value_ns).max(1), interval: ns_to_ticks(interval_ns) }
    };
    let left = if old.deadline == 0 { 0 } else { ticks_to_ns(old.deadline.saturating_sub(now).max(1)) };
    (left, ticks_to_ns(old.interval))
}

/// What is left of `p`'s real timer, and its interval, in nanoseconds
/// (`kern_getitimer`).
pub fn kern_getitimer_real(p: &Proc) -> (u64, u64) {
    let t = *p.itimer.lock();
    let left = if t.deadline == 0 { 0 } else { ticks_to_ns(t.deadline.saturating_sub(ticks()).max(1)) };
    (left, ticks_to_ns(t.interval))
}

/// Woken on every tick, for the timer thread.
static ITIMER_WAIT: WaitQueue = WaitQueue::new();

/// The thread that fires real-time interval timers (`realitexpire`, a
/// callout in FreeBSD): `SIGALRM` to each process whose timer is due.
pub fn itimer_thread() -> ! {
    loop {
        let seen = ticks();
        ITIMER_WAIT.wait_until(|| ticks() != seen);
        let now = ticks();
        foreach_proc(|q| {
            let fire = {
                let mut t = q.itimer.lock();
                if t.deadline != 0 && now >= t.deadline {
                    t.deadline = if t.interval == 0 { 0 } else { now + t.interval };
                    true
                } else {
                    false
                }
            };
            if fire {
                crate::kern_sig::psignal(q, Signal::SIGALRM);
            }
        });
    }
}

/// Called on every tick (from `hardclock`).
pub(crate) fn itimer_tick() {
    ITIMER_WAIT.wake_all();
}
