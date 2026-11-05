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


//! FreeBSD's `poll`, `ppoll`, `select` and `pselect`, on the kernel's
//! `kern_poll`.

use framework::mm::{KVec, M_WAITOK};
use kern::kern_exec::vm_errno;
use kern::kern_malloc::M_TEMP;
use kern::kern_proc::Proc;
use kern::kern_sig::{SigHow, kern_sigprocmask};
use kern::sys_generic::{PollFd, kern_poll};
use libkern::conv::{u64_from_usize, usize_from_u64};
use sys::errno::{Errno, KResult};

use crate::freebsd_signal::{SIGSET_SIZE, sigset_in};

const POLLIN: i16 = 0x1;
const POLLPRI: i16 = 0x2;
const POLLOUT: i16 = 0x4;
const POLLHUP: i16 = 0x10;
const POLLNVAL: i16 = 0x20;
const POLLRDNORM: i16 = 0x40;
const POLLWRNORM: i16 = POLLOUT;
const POLLRDBAND: i16 = 0x80;
const POLLWRBAND: i16 = 0x100;

/// Most descriptors one call watches.
const MAX_FDS: u64 = 1024;

/// A `struct timespec` at `ts` (0: none) as nanoseconds.
fn timespec_in(p: &Proc, ts: u64) -> KResult<Option<u64>> {
    if ts == 0 {
        return Ok(None);
    }
    let mut b = [0u8; 16];
    p.vm().copyin(ts, &mut b).map_err(vm_errno)?;
    let sec = i64::from_le_bytes(b[..8].try_into().map_err(|_| Errno::EFAULT)?);
    let nsec = i64::from_le_bytes(b[8..].try_into().map_err(|_| Errno::EFAULT)?);
    if sec < 0 || !(0..1_000_000_000).contains(&nsec) {
        return Err(Errno::EINVAL);
    }
    Ok(Some(u64::try_from(sec).unwrap_or(0).saturating_mul(1_000_000_000) + u64::try_from(nsec).unwrap_or(0)))
}

/// With `mask` (if any) as the signal mask while waiting, as `ppoll` and
/// `pselect` do.
fn with_mask<R>(p: &Proc, mask: u64, f: impl FnOnce() -> KResult<R>) -> KResult<R> {
    if mask == 0 {
        return f();
    }
    let mut b = [0u8; SIGSET_SIZE];
    p.vm().copyin(mask, &mut b).map_err(vm_errno)?;
    let old = kern_sigprocmask(p, SigHow::SetMask, Some(sigset_in(&b)));
    let r = f();
    kern_sigprocmask(p, SigHow::SetMask, Some(old));
    r
}

fn poll_common(p: &Proc, fds: u64, nfds: u64, timeout: Option<u64>) -> KResult<[u64; 2]> {
    if nfds > MAX_FDS {
        return Err(Errno::EINVAL);
    }
    let vm = p.vm();
    let n = usize_from_u64(nfds);
    let mut v: KVec<PollFd> = KVec::with_capacity(n, &M_TEMP, M_WAITOK)?;
    let mut events: KVec<i16> = KVec::with_capacity(n, &M_TEMP, M_WAITOK)?;
    for i in 0..nfds {
        let mut b = [0u8; 8];
        vm.copyin(fds + 8 * i, &mut b).map_err(vm_errno)?;
        let fd = i32::from_le_bytes([b[0], b[1], b[2], b[3]]);
        let ev = i16::from_le_bytes([b[4], b[5]]);
        v.push(
            PollFd {
                fd,
                read: ev & (POLLIN | POLLRDNORM | POLLRDBAND | POLLPRI) != 0,
                write: ev & (POLLOUT | POLLWRNORM | POLLWRBAND) != 0,
                ..PollFd::default()
            },
            M_WAITOK,
        )?;
        events.push(ev, M_WAITOK)?;
    }
    let ready = kern_poll(p, &mut v, timeout)?;
    for (i, (f, ev)) in v.iter().zip(events.iter()).enumerate() {
        let mut r = 0i16;
        if f.readable {
            r |= ev & (POLLIN | POLLRDNORM | POLLRDBAND | POLLPRI);
        }
        if f.writable {
            r |= ev & (POLLOUT | POLLWRNORM | POLLWRBAND);
        }
        if f.hup {
            r |= POLLHUP;
        }
        if f.invalid {
            r |= POLLNVAL;
        }
        vm.copyout(fds + 8 * u64_from_usize(i) + 6, &r.to_le_bytes()).map_err(vm_errno)?;
    }
    Ok([u64_from_usize(ready), 0])
}

/// `poll(fds, nfds, timeout)`: milliseconds, negative for none.
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_poll(p: &Proc, fds: u64, nfds: u64, timeout: i32) -> KResult<[u64; 2]> {
    let t = u64::try_from(timeout).ok().map(|ms| ms * 1_000_000);
    poll_common(p, fds, nfds, t)
}

/// `ppoll(fds, nfds, ts, set)`.
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_ppoll(p: &Proc, fds: u64, nfds: u64, ts: u64, set: u64) -> KResult<[u64; 2]> {
    let t = timespec_in(p, ts)?;
    with_mask(p, set, || poll_common(p, fds, nfds, t))
}

fn select_common(p: &Proc, nd: i32, sets: [u64; 3], timeout: Option<u64>) -> KResult<[u64; 2]> {
    let nd = usize::try_from(nd).map_err(|_| Errno::EINVAL)?.min(usize_from_u64(MAX_FDS));
    let words = nd.div_ceil(64);
    let vm = p.vm();
    // The three bit sets, read in: 64 descriptors a word.
    let mut bits: KVec<u64> = KVec::with_capacity(3 * words, &M_TEMP, M_WAITOK)?;
    for set in sets {
        for w in 0..words {
            let mut b = [0u8; 8];
            if set != 0 {
                vm.copyin(set + 8 * u64_from_usize(w), &mut b).map_err(vm_errno)?;
            }
            bits.push(u64::from_le_bytes(b), M_WAITOK)?;
        }
    }
    let has = |k: usize, fd: usize| bits.get(k * words + fd / 64).is_some_and(|w| w & (1 << (fd % 64)) != 0);
    let mut v: KVec<PollFd> = KVec::new(&M_TEMP);
    for fd in 0..nd {
        let (r, w) = (has(0, fd), has(1, fd));
        if r || w {
            v.push(PollFd { fd: i32::try_from(fd).unwrap_or(-1), read: r, write: w, ..PollFd::default() }, M_WAITOK)?;
        }
    }
    kern_poll(p, &mut v, timeout)?;
    if v.iter().any(|f| f.invalid) {
        return Err(Errno::EBADF);
    }
    // The answers, written back over the sets.
    let mut out: KVec<u64> = KVec::with_capacity(3 * words, &M_TEMP, M_WAITOK)?;
    for _ in 0..3 * words {
        out.push(0, M_WAITOK)?;
    }
    let mut count = 0u64;
    for f in v.iter() {
        let fd = usize::try_from(f.fd).unwrap_or(0);
        let mut mark = |k: usize| {
            if let Some(w) = out.get_mut(k * words + fd / 64) {
                *w |= 1 << (fd % 64);
            }
            count += 1;
        };
        if f.readable || (f.read && f.hup) {
            mark(0);
        }
        if f.writable {
            mark(1);
        }
    }
    for (k, set) in sets.into_iter().enumerate() {
        if set == 0 {
            continue;
        }
        for w in 0..words {
            let word = out.get(k * words + w).copied().unwrap_or(0);
            vm.copyout(set + 8 * u64_from_usize(w), &word.to_le_bytes()).map_err(vm_errno)?;
        }
    }
    Ok([count, 0])
}

/// `select(nd, in, ou, ex, tv)`: a `struct timeval`, or none.
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_select(p: &Proc, args: &[u64; 6]) -> KResult<[u64; 2]> {
    let t = if args[4] == 0 {
        None
    } else {
        let mut b = [0u8; 16];
        p.vm().copyin(args[4], &mut b).map_err(vm_errno)?;
        let sec = i64::from_le_bytes(b[..8].try_into().map_err(|_| Errno::EFAULT)?);
        let usec = i64::from_le_bytes(b[8..].try_into().map_err(|_| Errno::EFAULT)?);
        if sec < 0 || !(0..1_000_000).contains(&usec) {
            return Err(Errno::EINVAL);
        }
        Some(u64::try_from(sec).unwrap_or(0).saturating_mul(1_000_000_000) + u64::try_from(usec).unwrap_or(0) * 1000)
    };
    select_common(p, libkern::conv::trunc_i32(args[0]), [args[1], args[2], args[3]], t)
}

/// `pselect(nd, in, ou, ex, ts, sm)`.
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_pselect(p: &Proc, args: &[u64; 6]) -> KResult<[u64; 2]> {
    let t = timespec_in(p, args[4])?;
    with_mask(p, args[5], || select_common(p, libkern::conv::trunc_i32(args[0]), [args[1], args[2], args[3]], t))
}
