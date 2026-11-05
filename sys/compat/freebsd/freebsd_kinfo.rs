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


//! `kern.proc`: FreeBSD's `struct kinfo_proc` for each process, as `ps`,
//! `pgrep` and `top` read it (`fill_kinfo_proc`, `sysctl_kern_proc`),
//! built from the kernel's `ProcInfo`. Offsets from FreeBSD's
//! `sys/user.h`, checked with the host's compiler.

use framework::mm::{KVec, M_WAITOK};
use kern::kern_exec::vm_errno;
use kern::kern_malloc::M_TEMP;
use kern::kern_proc::{Proc, ProcInfo, ProcState, foreach_proc, pfind};
use libkern::conv::{u64_from_usize, usize_from_u64};
use sys::errno::{Errno, KResult};

/// `sizeof(struct kinfo_proc)` (`KINFO_PROC_SIZE`).
pub(crate) const KINFO_PROC_SIZE: usize = 1088;

const KI_PID: usize = 72;
const KI_PPID: usize = 76;
const KI_PGID: usize = 80;
const KI_TPGID: usize = 84;
const KI_SID: usize = 88;
const KI_TSID: usize = 92;
const KI_TDEV_FREEBSD11: usize = 100;
const KI_UID: usize = 168;
const KI_RUID: usize = 172;
const KI_SVUID: usize = 176;
const KI_RGID: usize = 180;
const KI_SVGID: usize = 184;
const KI_NGROUPS: usize = 188;
const KI_GROUPS: usize = 192;
const KI_SIZE: usize = 256;
const KI_RSSIZE: usize = 264;
const KI_START: usize = 336;
const KI_FLAG: usize = 368;
const KI_KIFLAG: usize = 376;
const KI_STAT: usize = 388;
const KI_NICE: usize = 389;
const KI_TDNAME: usize = 394;
const KI_LOGIN: usize = 420;
const KI_COMM: usize = 447;
const KI_EMUL: usize = 467;
const KI_LOGINCLASS: usize = 484;
const KI_TDEV: usize = 560;
const KI_NUMTHREADS: usize = 596;
const KI_TID: usize = 600;
const KI_PRI: usize = 604;
const KI_TDFLAGS: usize = 1080;
/// `TDF_SINTR`: the sleep is interruptible (every Nova sleep a process
/// does is).
const TDF_SINTR: u64 = 0x8;

const SRUN: u8 = 2;
const SSLEEP: u8 = 3;
const SSTOP: u8 = 4;
const SZOMB: u8 = 5;
const P_CONTROLT: u64 = 0x2;
const P_INMEM: u64 = 0x1000_0000;
const KI_CTTY: u64 = 0x1;
const KI_SLEADER: u64 = 0x2;
const NODEV: u64 = u64::MAX;
/// `PRI_TIMESHARE` and the user priority it starts at.
const PRI_TIMESHARE: u8 = 3;
const PUSER: u8 = 120;

fn put(b: &mut [u8], off: usize, v: &[u8]) {
    if let Some(d) = b.get_mut(off..off + v.len()) {
        d.copy_from_slice(v);
    }
}

/// A NUL-terminated string field of `size` bytes.
fn put_str(b: &mut [u8], off: usize, size: usize, s: &[u8]) {
    let n = s.len().min(size - 1);
    put(b, off, s.get(..n).unwrap_or(&[]));
}

/// The `kinfo_proc` of `i`.
fn kinfo_out(i: &ProcInfo, login: &[u8]) -> [u8; KINFO_PROC_SIZE] {
    let mut b = [0u8; KINFO_PROC_SIZE];
    put(&mut b, 0, &u32::try_from(KINFO_PROC_SIZE).unwrap_or(0).to_le_bytes());
    put(&mut b, KI_PID, &i.pid.to_le_bytes());
    put(&mut b, KI_PPID, &i.ppid.to_le_bytes());
    put(&mut b, KI_PGID, &i.pgid.to_le_bytes());
    put(&mut b, KI_SID, &i.sid.to_le_bytes());
    let (tdev, tpgid, tsid) = i.tty.map_or((NODEV, 0, 0), |(d, g)| (d, g, i.sid));
    put(&mut b, KI_TPGID, &tpgid.to_le_bytes());
    put(&mut b, KI_TSID, &tsid.to_le_bytes());
    put(&mut b, KI_TDEV_FREEBSD11, &u32::try_from(tdev).unwrap_or(u32::MAX).to_le_bytes());
    put(&mut b, KI_TDEV, &tdev.to_le_bytes());
    let c = &i.cred;
    put(&mut b, KI_UID, &c.euid.to_le_bytes());
    put(&mut b, KI_RUID, &c.ruid.to_le_bytes());
    put(&mut b, KI_SVUID, &c.svuid.to_le_bytes());
    put(&mut b, KI_RGID, &c.rgid.to_le_bytes());
    put(&mut b, KI_SVGID, &c.svgid.to_le_bytes());
    let groups = c.groups();
    let ng = groups.len().min(16);
    put(&mut b, KI_NGROUPS, &u16::try_from(ng).unwrap_or(0).to_le_bytes());
    for (k, g) in groups.iter().take(ng).enumerate() {
        put(&mut b, KI_GROUPS + 4 * k, &g.to_le_bytes());
    }
    put(&mut b, KI_SIZE, &i.vsize.to_le_bytes());
    put(&mut b, KI_RSSIZE, &(i.rss / u64_from_usize(framework::PAGE_SIZE)).to_le_bytes());
    // When it started: boot time plus its age at creation.
    let hz = u64::from(kern::kern_clock::hz().max(1));
    let start_us = i.start_ticks * (1_000_000 / hz);
    put(&mut b, KI_START, &(kern::kern_clock::boottime() + start_us / 1_000_000).to_le_bytes());
    put(&mut b, KI_START + 8, &(start_us % 1_000_000).to_le_bytes());
    let ctty = i.tty.is_some();
    put(&mut b, KI_FLAG, &(P_INMEM | if ctty { P_CONTROLT } else { 0 }).to_le_bytes());
    let kiflag = (if ctty { KI_CTTY } else { 0 }) | if i.pid == i.sid { KI_SLEADER } else { 0 };
    put(&mut b, KI_KIFLAG, &kiflag.to_le_bytes());
    if i.state == ProcState::Sleep {
        put(&mut b, KI_TDFLAGS, &TDF_SINTR.to_le_bytes());
    }
    b[KI_STAT] = match i.state {
        ProcState::Run => SRUN,
        ProcState::Sleep => SSLEEP,
        ProcState::Stop => SSTOP,
        ProcState::Zombie => SZOMB,
    };
    b[KI_NICE] = i8::try_from(i.nice).unwrap_or(0).to_le_bytes()[0];
    put_str(&mut b, KI_TDNAME, 17, i.comm.as_str().as_bytes());
    put_str(&mut b, KI_LOGIN, 18, login);
    put_str(&mut b, KI_COMM, 20, i.comm.as_str().as_bytes());
    put_str(&mut b, KI_EMUL, 17, i.abi_name.as_bytes());
    put_str(&mut b, KI_LOGINCLASS, 18, i.loginclass.as_str().as_bytes());
    put(&mut b, KI_NUMTHREADS, &1u32.to_le_bytes());
    put(&mut b, KI_TID, &u32::try_from(i.tid).unwrap_or(0).to_le_bytes());
    b[KI_PRI] = PRI_TIMESHARE;
    b[KI_PRI + 1] = PUSER;
    b[KI_PRI + 2] = PUSER;
    b[KI_PRI + 3] = PUSER;
    b
}

/// `kern.proc.<op>.<arg>`: the `kinfo_proc` of the processes `op` selects
/// (`KERN_PROC_ALL`, `PROC`, `PID`, `PGRP`, `SESSION`, `TTY`, `UID`,
/// `RUID`), or `ENOMEM` with what fits.
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sysctl_kern_proc(p: &Proc, op: i32, arg: Option<i32>, old: u64, oldlenp: u64) -> KResult<[u64; 2]> {
    let op = op & !0x10; // KERN_PROC_INC_THREAD: one thread each anyway
    let arg_u = arg.and_then(|a| u32::try_from(a).ok());
    let arg_dev = arg.map(|a| u64::from(a.unsigned_abs()));
    let wanted = |i: &ProcInfo| match op {
        0 | 8 => true,
        1 => arg_u == Some(i.pid),
        2 => arg_u == Some(i.pgid),
        3 => arg_u == Some(i.sid),
        4 => i.tty.map(|t| t.0) == arg_dev,
        5 => arg_u == Some(i.cred.euid),
        6 => arg_u == Some(i.cred.ruid),
        _ => false,
    };
    if !matches!(op, 0..=6 | 8) {
        return Err(Errno::EINVAL);
    }
    // A snapshot first: copying out may sleep.
    let mut infos: KVec<ProcInfo> = KVec::new(&M_TEMP);
    let mut err = None;
    foreach_proc(|q| {
        let i = q.info();
        if wanted(&i) && err.is_none() && let Err(e) = infos.push(i, M_WAITOK) {
            err = Some(Errno::from(e));
        }
    });
    if let Some(e) = err {
        return Err(e);
    }
    if op == 1 && infos.is_empty() {
        return Err(Errno::ESRCH);
    }
    let (lb, ln) = *crate::freebsd_proc::LOGIN.lock();
    let login = lb.get(..ln).unwrap_or(&[]);
    let vm = p.vm();
    let need = infos.len() * KINFO_PROC_SIZE;
    let mut lenb = [0u8; 8];
    vm.copyin(oldlenp, &mut lenb).map_err(vm_errno)?;
    let room = usize_from_u64(u64::from_le_bytes(lenb));
    if old == 0 {
        // Room for a few more, as FreeBSD estimates (`SESSION` may grow).
        vm.copyout(oldlenp, &u64_from_usize(need + 4 * KINFO_PROC_SIZE).to_le_bytes()).map_err(vm_errno)?;
        return Ok([0, 0]);
    }
    let mut done = 0;
    for i in infos.iter() {
        if done + KINFO_PROC_SIZE > room {
            break;
        }
        vm.copyout(old + u64_from_usize(done), &kinfo_out(i, login)).map_err(vm_errno)?;
        done += KINFO_PROC_SIZE;
    }
    vm.copyout(oldlenp, &u64_from_usize(done).to_le_bytes()).map_err(vm_errno)?;
    if done < need { Err(Errno::ENOMEM) } else { Ok([0, 0]) }
}

/// `kern.proc.args.<pid>`: the arguments of a process, each with its NUL.
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sysctl_kern_proc_args(p: &Proc, pid: i32, old: u64, oldlenp: u64) -> KResult<[u64; 2]> {
    let target = if pid == -1 { None } else { Some(pfind(u32::try_from(pid).map_err(|_| Errno::EINVAL)?).ok_or(Errno::ESRCH)?) };
    let q: &Proc = target.as_deref().unwrap_or(p);
    let mut buf: KVec<u8> = KVec::new(&M_TEMP);
    q.with_args(|a| buf.extend_from_slice(a, M_WAITOK))?;
    crate::freebsd_sysctl::reply(p, &buf, old, oldlenp)
}
