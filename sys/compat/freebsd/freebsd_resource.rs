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


//! FreeBSD's resource and time calls: `getrlimit`/`setrlimit` (with
//! FreeBSD's `RLIMIT_*` numbers and `RLIM_INFINITY`), priorities, login
//! classes, interval timers, `gettimeofday`/`settimeofday`.

use kern::kern_clock::{ClockId, kern_clock_gettime, settime};
use kern::kern_exec::vm_errno;
use kern::kern_proc::{Comm, Proc};
use kern::kern_resource::{
    PrioWho, RLIM_INFINITY, Resource, Rlimit, kern_getitimer_real, kern_getpriority, kern_getrlimit,
    kern_setitimer_real, kern_setpriority, kern_setrlimit,
};
use sys::errno::{Errno, KResult};

/// FreeBSD's `RLIM_INFINITY`: the largest `rlim_t`.
const FREEBSD_RLIM_INFINITY: u64 = 0x7fff_ffff_ffff_ffff;

fn resource(n: i32) -> KResult<Resource> {
    Ok(match n {
        0 => Resource::Cpu,
        1 => Resource::FileSize,
        2 => Resource::Data,
        3 => Resource::Stack,
        4 => Resource::Core,
        5 => Resource::Rss,
        6 => Resource::MemLock,
        7 => Resource::NProc,
        8 => Resource::NoFile,
        9 => Resource::SbSize,
        10 => Resource::VMem,
        11 => Resource::Npts,
        12 => Resource::Swap,
        13 => Resource::Kqueues,
        14 => Resource::Umtxp,
        15 => Resource::PipeBuf,
        _ => return Err(Errno::EINVAL),
    })
}

fn to_user(v: u64) -> u64 {
    if v == RLIM_INFINITY { FREEBSD_RLIM_INFINITY } else { v.min(FREEBSD_RLIM_INFINITY) }
}

fn from_user(v: u64) -> u64 {
    if v >= FREEBSD_RLIM_INFINITY { RLIM_INFINITY } else { v }
}

/// `getrlimit(which, rlp)`.
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_getrlimit(p: &Proc, which: i32, rlp: u64) -> KResult<[u64; 2]> {
    let l = kern_getrlimit(p, resource(which)?);
    let mut b = [0u8; 16];
    b[..8].copy_from_slice(&to_user(l.cur).to_le_bytes());
    b[8..].copy_from_slice(&to_user(l.max).to_le_bytes());
    p.vm().copyout(rlp, &b).map_err(vm_errno)?;
    Ok([0, 0])
}

/// `setrlimit(which, rlp)`.
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_setrlimit(p: &Proc, which: i32, rlp: u64) -> KResult<[u64; 2]> {
    let mut b = [0u8; 16];
    p.vm().copyin(rlp, &mut b).map_err(vm_errno)?;
    let cur = u64::from_le_bytes(b[..8].try_into().map_err(|_| Errno::EFAULT)?);
    let max = u64::from_le_bytes(b[8..].try_into().map_err(|_| Errno::EFAULT)?);
    kern_setrlimit(p, resource(which)?, Rlimit { cur: from_user(cur), max: from_user(max) })?;
    Ok([0, 0])
}

fn prio_who(which: i32, who: i32) -> KResult<PrioWho> {
    let who = u32::try_from(who).map_err(|_| Errno::ESRCH)?;
    Ok(match which {
        0 => PrioWho::Process(who),
        1 => PrioWho::Group(who),
        2 => PrioWho::User(who),
        _ => return Err(Errno::EINVAL),
    })
}

/// `getpriority(which, who)`: the nice value itself (it may be negative).
pub(crate) fn sys_getpriority(p: &Proc, which: i32, who: i32) -> KResult<[u64; 2]> {
    let n = kern_getpriority(p, prio_who(which, who)?)?;
    Ok([u64::from_le_bytes(i64::from(n).to_le_bytes()), 0])
}

/// `setpriority(which, who, prio)`.
pub(crate) fn sys_setpriority(p: &Proc, which: i32, who: i32, prio: i32) -> KResult<[u64; 2]> {
    kern_setpriority(p, prio_who(which, who)?, prio)?;
    Ok([0, 0])
}

/// `setloginclass(name)`.
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_setloginclass(p: &Proc, name: u64) -> KResult<[u64; 2]> {
    let mut b = [0u8; 33];
    let n = crate::freebsd_vfs::copyinstr(&p.vm(), name, &mut b).map_err(|e| if e == Errno::ENAMETOOLONG { Errno::EINVAL } else { e })?;
    let name = core::str::from_utf8(&b[..n]).map_err(|_| Errno::EINVAL)?;
    *p.loginclass.lock() = Comm::from_path(name);
    Ok([0, 0])
}

/// `getloginclass(name, len)`.
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_getloginclass(p: &Proc, buf: u64, len: u64) -> KResult<[u64; 2]> {
    let c = *p.loginclass.lock();
    let s = c.as_str().as_bytes();
    if len < u64::try_from(s.len() + 1).unwrap_or(u64::MAX) {
        return Err(Errno::ERANGE);
    }
    let vm = p.vm();
    vm.copyout(buf, s).map_err(vm_errno)?;
    vm.copyout(buf + u64::try_from(s.len()).unwrap_or(0), &[0]).map_err(vm_errno)?;
    Ok([0, 0])
}

fn tv_out(ns: u64) -> [u8; 16] {
    let mut b = [0u8; 16];
    b[..8].copy_from_slice(&(ns / 1_000_000_000).to_le_bytes());
    b[8..].copy_from_slice(&((ns % 1_000_000_000) / 1000).to_le_bytes());
    b
}

fn tv_in(b: &[u8]) -> KResult<u64> {
    let sec = i64::from_le_bytes(b.get(..8).and_then(|s| s.try_into().ok()).ok_or(Errno::EFAULT)?);
    let usec = i64::from_le_bytes(b.get(8..16).and_then(|s| s.try_into().ok()).ok_or(Errno::EFAULT)?);
    if sec < 0 || !(0..1_000_000).contains(&usec) {
        return Err(Errno::EINVAL);
    }
    Ok(u64::try_from(sec).unwrap_or(0).saturating_mul(1_000_000_000) + u64::try_from(usec).unwrap_or(0) * 1000)
}

/// `setitimer(which, itv, oitv)`: `ITIMER_REAL`; the virtual and
/// profiling timers are accepted and never fire. TODO(resource).
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_setitimer(p: &Proc, which: i32, itv: u64, oitv: u64) -> KResult<[u64; 2]> {
    const ITIMER_REAL: i32 = 0;
    if !(0..=2).contains(&which) {
        return Err(Errno::EINVAL);
    }
    let vm = p.vm();
    let new = if itv == 0 {
        None
    } else {
        let mut b = [0u8; 32];
        vm.copyin(itv, &mut b).map_err(vm_errno)?;
        Some((tv_in(&b[16..])?, tv_in(&b[..16])?))
    };
    let old = if which == ITIMER_REAL {
        match new {
            Some((value, interval)) => kern_setitimer_real(p, value, interval),
            None => kern_getitimer_real(p),
        }
    } else {
        (0, 0)
    };
    if oitv != 0 {
        let mut b = [0u8; 32];
        b[..16].copy_from_slice(&tv_out(old.1));
        b[16..].copy_from_slice(&tv_out(old.0));
        vm.copyout(oitv, &b).map_err(vm_errno)?;
    }
    Ok([0, 0])
}

/// `getitimer(which, itv)`.
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_getitimer(p: &Proc, which: i32, itv: u64) -> KResult<[u64; 2]> {
    if !(0..=2).contains(&which) {
        return Err(Errno::EINVAL);
    }
    let (value, interval) = if which == 0 { kern_getitimer_real(p) } else { (0, 0) };
    let mut b = [0u8; 32];
    b[..16].copy_from_slice(&tv_out(interval));
    b[16..].copy_from_slice(&tv_out(value));
    p.vm().copyout(itv, &b).map_err(vm_errno)?;
    Ok([0, 0])
}

/// `gettimeofday(tp, tzp)`: the wall clock; the time zone is UTC.
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_gettimeofday(p: &Proc, tp: u64, tzp: u64) -> KResult<[u64; 2]> {
    let vm = p.vm();
    if tp != 0 {
        let t = kern_clock_gettime(ClockId::Realtime);
        let mut b = [0u8; 16];
        b[..8].copy_from_slice(&t.sec.to_le_bytes());
        b[8..].copy_from_slice(&(t.nsec / 1000).to_le_bytes());
        vm.copyout(tp, &b).map_err(vm_errno)?;
    }
    if tzp != 0 {
        vm.copyout(tzp, &[0u8; 8]).map_err(vm_errno)?;
    }
    Ok([0, 0])
}

/// `settimeofday(tp, tzp)`: as root, sets the wall clock.
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_settimeofday(p: &Proc, tp: u64) -> KResult<[u64; 2]> {
    if tp != 0 {
        let mut b = [0u8; 16];
        p.vm().copyin(tp, &mut b).map_err(vm_errno)?;
        settime(tv_in(&b)? / 1_000_000_000);
    }
    Ok([0, 0])
}

/// `clock_settime(clock_id, tp)`: only `CLOCK_REALTIME`.
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_clock_settime(p: &Proc, id: i32, tp: u64) -> KResult<[u64; 2]> {
    if id != 0 {
        return Err(Errno::EINVAL);
    }
    let mut b = [0u8; 16];
    p.vm().copyin(tp, &mut b).map_err(vm_errno)?;
    let sec = i64::from_le_bytes(b[..8].try_into().map_err(|_| Errno::EFAULT)?);
    settime(u64::try_from(sec).map_err(|_| Errno::EINVAL)?);
    Ok([0, 0])
}
