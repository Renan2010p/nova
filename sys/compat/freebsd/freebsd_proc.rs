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


//! FreeBSD's process calls: `fork`, `execve`, `wait4`/`wait6`, process
//! groups and sessions, `kill`. The status words and the child's return
//! values are FreeBSD's; the work is the kernel's.

use framework::mm::{KVec, M_WAITOK};
use framework::user::UserRegs;
use kern::kern_exec::{kern_execve_path, vm_errno};
use kern::kern_exit::ExitStatus;
use kern::kern_fork::kern_fork;
use kern::kern_malloc::M_TEMP;
use kern::kern_proc::{
    Proc, StopEvent, WaitEvent, WaitOptions, WaitWho, kern_getpgid, kern_getsid, kern_setpgid, kern_setsid, kern_wait,
};
use kern::kern_sig::{KillWho, SigHow, kern_kill, kern_sigpending, kern_sigprocmask, kern_sigsuspend};
use sys::errno::{Errno, KResult};

use crate::arch::{cpu_set_syscall_retval, getcontext, sigreturn};
use crate::freebsd_signal::{SIGSET_SIZE, from_signo, sigset_in, sigset_out, signo};
use crate::freebsd_sysent::SyscallResult;
use crate::freebsd_vfs::copyinstr;

/// `fork()` (and `vfork()`, which may be a fork): the child returns 0,
/// with 1 in `%rdx` (`cpu_fork`).
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_fork(p: &Proc, regs: &UserRegs) -> KResult<[u64; 2]> {
    let mut child = *regs;
    cpu_set_syscall_retval(&mut child, Ok([0, 1]));
    let pid = kern_fork(p, child)?;
    Ok([u64::from(pid), 0])
}

/// Most bytes of arguments and environment (`ARG_MAX`).
const ARG_MAX: usize = 256 * 1024;
/// Most arguments and environment strings.
const ARG_COUNT_MAX: usize = 16 * 1024;

/// Copies in a NULL-terminated vector of strings at `vec`, appending the
/// strings (with their NULs) to `blob` and their spans to `spans`.
fn copyin_vector(p: &Proc, vec: u64, blob: &mut KVec<u8>, spans: &mut KVec<(usize, usize)>) -> KResult<()> {
    if vec == 0 {
        return Ok(());
    }
    let vm = p.vm();
    let mut s: KVec<u8> = KVec::with_capacity(4096, &M_TEMP, M_WAITOK)?;
    for _ in 0..4096 {
        s.push(0, M_WAITOK)?;
    }
    for i in 0.. {
        if spans.len() >= ARG_COUNT_MAX {
            return Err(Errno::E2BIG);
        }
        let mut ptr = [0u8; 8];
        vm.copyin(vec + 8 * i, &mut ptr).map_err(vm_errno)?;
        let addr = u64::from_le_bytes(ptr);
        if addr == 0 {
            return Ok(());
        }
        let n = copyinstr(&vm, addr, &mut s).map_err(|e| if e == Errno::ENAMETOOLONG { Errno::E2BIG } else { e })?;
        if blob.len() + n + 1 > ARG_MAX {
            return Err(Errno::E2BIG);
        }
        let start = blob.len();
        blob.extend_from_slice(s.get(..n).unwrap_or(&[]), M_WAITOK)?;
        spans.push((start, n), M_WAITOK)?;
    }
    Ok(())
}

/// `execve(path, argv, envv)`: on success the registers are the new
/// image's, returned as they are (`EJUSTRETURN`).
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_execve(p: &Proc, regs: &mut UserRegs, path: u64, argv: u64, envv: u64) -> SyscallResult {
    let r = (|| -> KResult<UserRegs> {
        let mut name = [0u8; 1024];
        let n = copyinstr(&p.vm(), path, &mut name)?;
        let mut blob: KVec<u8> = KVec::with_capacity(4096, &M_TEMP, M_WAITOK)?;
        let mut args: KVec<(usize, usize)> = KVec::new(&M_TEMP);
        let mut envs: KVec<(usize, usize)> = KVec::new(&M_TEMP);
        copyin_vector(p, argv, &mut blob, &mut args)?;
        copyin_vector(p, envv, &mut blob, &mut envs)?;
        let mut argv_s: KVec<&str> = KVec::with_capacity(args.len(), &M_TEMP, M_WAITOK)?;
        let mut envv_s: KVec<&str> = KVec::with_capacity(envs.len(), &M_TEMP, M_WAITOK)?;
        for &(start, len) in args.iter() {
            let s = blob.get(start..start + len).ok_or(Errno::EFAULT)?;
            argv_s.push(core::str::from_utf8(s).map_err(|_| Errno::EINVAL)?, M_WAITOK)?;
        }
        for &(start, len) in envs.iter() {
            let s = blob.get(start..start + len).ok_or(Errno::EFAULT)?;
            envv_s.push(core::str::from_utf8(s).map_err(|_| Errno::EINVAL)?, M_WAITOK)?;
        }
        kern_execve_path(p, name.get(..n).unwrap_or(&[]), &argv_s, &envv_s)
    })();
    match r {
        Ok(new) => {
            *regs = new;
            SyscallResult::JustReturn
        }
        Err(e) => SyscallResult::Err(e),
    }
}

/// FreeBSD's status word (`W_EXITCODE`, `W_TERMSIG`, `W_STOPCODE`, and
/// `0x13` for a continued process).
fn status_word(ev: WaitEvent) -> i32 {
    match ev {
        WaitEvent::Exited(ExitStatus::Exited(rv)) => (rv & 0xff) << 8,
        WaitEvent::Exited(ExitStatus::Signaled(sig)) => signo(sig) & 0x7f,
        WaitEvent::Changed(StopEvent::Stopped(sig)) => (signo(sig) << 8) | 0x7f,
        WaitEvent::Changed(StopEvent::Continued) => 0x13,
    }
}

const WNOHANG: i32 = 1;

/// Waits as `wait4`/`wait6` do, and copies out the status and an empty
/// resource usage of `rusage_size` bytes.
fn wait_common(p: &Proc, who: WaitWho, status: u64, options: i32, rusage: u64, rusage_size: usize) -> KResult<[u64; 2]> {
    const WUNTRACED: i32 = 2;
    const WCONTINUED: i32 = 4;
    const WNOWAIT: i32 = 8;
    const WEXITED: i32 = 0x10;
    const WTRAPPED: i32 = 0x20;
    if options & !(WNOHANG | WUNTRACED | WCONTINUED | WNOWAIT | WEXITED | WTRAPPED) != 0 {
        return Err(Errno::EINVAL);
    }
    let opts = WaitOptions {
        nohang: options & WNOHANG != 0,
        stopped: options & (WUNTRACED | WTRAPPED) != 0,
        continued: options & WCONTINUED != 0,
        nowait: options & WNOWAIT != 0,
    };
    let Some((pid, ev)) = kern_wait(p, who, opts)? else { return Ok([0, 0]) };
    let vm = p.vm();
    if status != 0 {
        vm.copyout(status, &status_word(ev).to_le_bytes()).map_err(vm_errno)?;
    }
    if rusage != 0 {
        // TODO(rusage): resource usage.
        let zero = [0u8; 288];
        vm.copyout(rusage, zero.get(..rusage_size).unwrap_or(&zero)).map_err(vm_errno)?;
    }
    Ok([u64::from(pid), 0])
}

/// `wait4(pid, status, options, rusage)`.
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_wait4(p: &Proc, pid: i32, status: u64, options: i32, rusage: u64) -> KResult<[u64; 2]> {
    let who = match pid {
        -1 => WaitWho::Any,
        0 => WaitWho::Pgid(p.pgid()),
        n if n < -1 => WaitWho::Pgid(n.unsigned_abs()),
        n => WaitWho::Pid(n.unsigned_abs()),
    };
    wait_common(p, who, status, options, rusage, 144)
}

/// `wait6(idtype, id, status, options, wrusage, info)`.
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_wait6(p: &Proc, args: &[u64; 6]) -> KResult<[u64; 2]> {
    const P_PID: i32 = 0;
    const P_PGID: i32 = 2;
    const P_ALL: i32 = 7;
    let id = u32::try_from(args[1]).map_err(|_| Errno::EINVAL)?;
    let who = match libkern::conv::trunc_i32(args[0]) {
        P_PID => WaitWho::Pid(id),
        P_PGID => WaitWho::Pgid(if id == 0 { p.pgid() } else { id }),
        P_ALL => WaitWho::Any,
        _ => return Err(Errno::EINVAL),
    };
    let r = wait_common(p, who, args[2], libkern::conv::trunc_i32(args[3]), args[4], 288)?;
    if args[5] != 0 {
        // TODO(signal): siginfo_t of the child.
        p.vm().copyout(args[5], &[0u8; 80]).map_err(vm_errno)?;
    }
    Ok(r)
}

/// `setpgid(pid, pgid)`.
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_setpgid(p: &Proc, pid: i32, pgid: i32) -> KResult<[u64; 2]> {
    if pid < 0 || pgid < 0 {
        return Err(Errno::EINVAL);
    }
    kern_setpgid(p, pid.unsigned_abs(), pgid.unsigned_abs())?;
    Ok([0, 0])
}

/// `setsid()`.
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_setsid(p: &Proc) -> KResult<[u64; 2]> {
    Ok([u64::from(kern_setsid(p)?), 0])
}

/// `getpgid(pid)` and `getsid(pid)`.
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_getpgid(p: &Proc, pid: i32, session: bool) -> KResult<[u64; 2]> {
    let pid = u32::try_from(pid).map_err(|_| Errno::ESRCH)?;
    let id = if session { kern_getsid(p, pid)? } else { kern_getpgid(p, pid)? };
    Ok([u64::from(id), 0])
}

/// `kill(pid, sig)`.
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_kill(p: &Proc, pid: i32, sig: i32) -> SyscallResult {
    let sig = match sig {
        0 => None,
        n => match from_signo(n) {
            Some(s) => Some(s),
            None => return SyscallResult::Err(Errno::EINVAL),
        },
    };
    let who = match pid {
        0 => KillWho::OwnGroup,
        -1 => KillWho::All,
        n if n < -1 => KillWho::Group(n.unsigned_abs()),
        n => KillWho::Pid(n.unsigned_abs()),
    };
    match kern_kill(p, who, sig) {
        Ok(()) => SyscallResult::Ok([0, 0]),
        Err(e) => SyscallResult::Err(e),
    }
}

/// `umask(newmask)`: returns the old mask.
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_umask(p: &Proc, mask: u64) -> KResult<[u64; 2]> {
    let new = u32::try_from(mask & 0o777).map_err(|_| Errno::EINVAL)?;
    let old = p.umask.swap(new, core::sync::atomic::Ordering::Relaxed);
    Ok([u64::from(old), 0])
}

/// `pipe2(fildes, flags)`: `O_CLOEXEC`, `O_NONBLOCK`.
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_pipe2(p: &Proc, fds: u64, flags: i32) -> KResult<[u64; 2]> {
    const O_NONBLOCK: i32 = 0x4;
    const O_CLOEXEC: i32 = 0x10_0000;
    if flags & !(O_NONBLOCK | O_CLOEXEC) != 0 {
        return Err(Errno::EINVAL);
    }
    let (r, w) = kern::sys_pipe::kern_pipe(p, flags & O_CLOEXEC != 0, flags & O_NONBLOCK != 0)?;
    let mut b = [0u8; 8];
    b[..4].copy_from_slice(&r.to_le_bytes());
    b[4..].copy_from_slice(&w.to_le_bytes());
    if let Err(e) = p.vm().copyout(fds, &b) {
        let _ = p.fd.close(r);
        let _ = p.fd.close(w);
        return Err(vm_errno(e));
    }
    Ok([0, 0])
}

/// `sigreturn(ucp)`: back from a handler, as it was before
/// (`EJUSTRETURN`).
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_sigreturn(p: &Proc, regs: &mut UserRegs, ucp: u64) -> SyscallResult {
    match sigreturn(&p.vm(), ucp, regs) {
        Ok(mask) => {
            kern_sigprocmask(p, SigHow::SetMask, Some(mask));
            SyscallResult::JustReturn
        }
        Err(e) => SyscallResult::Err(e),
    }
}

/// `sigsuspend(sigmask)`: waits for a signal with that mask; the handler
/// runs with it, and the old mask comes back after (always `EINTR`).
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_sigsuspend(p: &Proc, set: u64) -> KResult<[u64; 2]> {
    let mut b = [0u8; SIGSET_SIZE];
    p.vm().copyin(set, &mut b).map_err(vm_errno)?;
    let old = kern_sigsuspend(p, sigset_in(&b));
    // The handler's frame takes the suspended mask; after it returns, the
    // old one must hold: put it back now, the frame keeps what to restore.
    // TODO(signal): `TDP_OLDMASK`, so the frame saves `old` itself.
    kern_sigprocmask(p, SigHow::SetMask, Some(old));
    Err(Errno::EINTR)
}

/// `sigpending(set)`.
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_sigpending(p: &Proc, set: u64) -> KResult<[u64; 2]> {
    p.vm().copyout(set, &sigset_out(kern_sigpending(p))).map_err(vm_errno)?;
    Ok([0, 0])
}

/// The login name of every session: one for now (`s_login`).
static LOGIN: framework::sync::MtxSpin<([u8; 33], usize)> = framework::sync::MtxSpin::new(([0; 33], 0));

/// `setlogin(name)`.
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_setlogin(p: &Proc, name: u64) -> KResult<[u64; 2]> {
    let mut b = [0u8; 33];
    let n = copyinstr(&p.vm(), name, &mut b).map_err(|e| if e == Errno::ENAMETOOLONG { Errno::EINVAL } else { e })?;
    *LOGIN.lock() = (b, n);
    Ok([0, 0])
}

/// `getlogin(name, len)`: the name with its NUL.
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_getlogin(p: &Proc, buf: u64, len: u64) -> KResult<[u64; 2]> {
    let (b, n) = *LOGIN.lock();
    let want = n + 1;
    if len < u64::try_from(want).unwrap_or(u64::MAX) {
        return Err(Errno::ERANGE);
    }
    p.vm().copyout(buf, b.get(..want).unwrap_or(&[0])).map_err(vm_errno)?;
    Ok([0, 0])
}

/// `getcontext(ucp)`.
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_getcontext(p: &Proc, regs: &UserRegs, ucp: u64) -> KResult<[u64; 2]> {
    let mask = kern_sigprocmask(p, SigHow::Block, None);
    getcontext(&p.vm(), regs, mask, ucp)?;
    Ok([0, 0])
}

/// `setcontext(ucp)`: as `sigreturn`.
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_setcontext(p: &Proc, regs: &mut UserRegs, ucp: u64) -> SyscallResult {
    sys_sigreturn(p, regs, ucp)
}

/// `swapcontext(oucp, ucp)`: saves to `oucp`, loads `ucp`.
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_swapcontext(p: &Proc, regs: &mut UserRegs, oucp: u64, ucp: u64) -> SyscallResult {
    if let Err(e) = sys_getcontext(p, regs, oucp) {
        return SyscallResult::Err(e);
    }
    sys_sigreturn(p, regs, ucp)
}

/// `thr_wake(id)`: for the caller, the next `thr_suspend` would not sleep
/// (`TDP_WAKEUP`); there is no other thread. TODO(thread).
pub(crate) fn sys_thr_wake(id: u64) -> KResult<[u64; 2]> {
    if id == kern::kern_thr::kern_thr_self() { Ok([0, 0]) } else { Err(Errno::ESRCH) }
}

/// `rtprio_thread(function, lwpid, rtp)`: every thread runs at the normal
/// time-sharing priority (`RTP_PRIO_NORMAL`, 0); setting it is accepted.
/// TODO(sched): real-time and idle classes.
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_rtprio_thread(p: &Proc, function: i32, lwpid: u64, rtp: u64) -> KResult<[u64; 2]> {
    const RTP_LOOKUP: i32 = 0;
    const RTP_SET: i32 = 1;
    const RTP_PRIO_NORMAL: u16 = 3;
    if lwpid != 0 && lwpid != kern::kern_thr::kern_thr_self() {
        return Err(Errno::ESRCH);
    }
    match function {
        RTP_LOOKUP => {
            let mut b = [0u8; 4];
            b[..2].copy_from_slice(&RTP_PRIO_NORMAL.to_le_bytes());
            p.vm().copyout(rtp, &b).map_err(vm_errno)?;
            Ok([0, 0])
        }
        RTP_SET => Ok([0, 0]),
        _ => Err(Errno::EINVAL),
    }
}

/// `_umtx_op(obj, op, val, uaddr, uaddr2)`, for one thread per process:
/// wakes find no one; a wait returns at once if the word changed, else
/// sleeps until a signal or its timeout.
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_umtx_op(p: &Proc, args: &[u64; 6]) -> KResult<[u64; 2]> {
    const UMTX_OP_WAIT: i32 = 2;
    const UMTX_OP_WAKE: i32 = 3;
    const UMTX_OP_WAIT_UINT: i32 = 11;
    const UMTX_OP_WAIT_UINT_PRIVATE: i32 = 15;
    const UMTX_OP_WAKE_PRIVATE: i32 = 16;
    const UMTX_OP_NWAKE_PRIVATE: i32 = 21;
    let (obj, op, val) = (args[0], libkern::conv::trunc_i32(args[1]), args[2]);
    let vm = p.vm();
    let wide = match op {
        UMTX_OP_WAKE | UMTX_OP_WAKE_PRIVATE | UMTX_OP_NWAKE_PRIVATE => return Ok([0, 0]),
        UMTX_OP_WAIT => true,
        UMTX_OP_WAIT_UINT | UMTX_OP_WAIT_UINT_PRIVATE => false,
        _ => {
            if kern::init_main::bootverbose() {
                kern::printf!("pid {} comm {}: _umtx_op {} not implemented\n", p.pid, p.comm(), op);
            }
            return Err(Errno::EINVAL);
        }
    };
    let cur = if wide {
        let mut b = [0u8; 8];
        vm.copyin(obj, &mut b).map_err(vm_errno)?;
        u64::from_le_bytes(b)
    } else {
        let mut b = [0u8; 4];
        vm.copyin(obj, &mut b).map_err(vm_errno)?;
        u64::from(u32::from_le_bytes(b))
    };
    let want = if wide { val } else { val & 0xffff_ffff };
    if cur != want {
        return Ok([0, 0]);
    }
    // The timeout: a `struct timespec` or a `struct _umtx_time` (whose
    // first member is one), relative.
    let timeout = if args[4] == 0 {
        None
    } else {
        let mut t = [0u8; 16];
        vm.copyin(args[4], &mut t).map_err(vm_errno)?;
        let sec = i64::from_le_bytes(t[..8].try_into().map_err(|_| Errno::EFAULT)?);
        let nsec = i64::from_le_bytes(t[8..].try_into().map_err(|_| Errno::EFAULT)?);
        if sec < 0 || !(0..1_000_000_000).contains(&nsec) {
            return Err(Errno::EINVAL);
        }
        Some(u64::try_from(sec).unwrap_or(0).saturating_mul(1_000_000_000).saturating_add(u64::try_from(nsec).unwrap_or(0)))
    };
    kern::kern_synch::kern_umtx_sleep(p, timeout)?;
    Ok([0, 0])
}
