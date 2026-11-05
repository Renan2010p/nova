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
use kern::kern_proc::{Proc, WaitWho, kern_getpgid, kern_getsid, kern_setpgid, kern_setsid, kern_wait};
use kern::kern_sig::{KillWho, SelfSignal, kern_kill};
use sys::errno::{Errno, KResult};

use crate::arch::cpu_set_syscall_retval;
use crate::freebsd_signal::{from_signo, signo};
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

/// FreeBSD's status word (`W_EXITCODE`, `W_TERMSIG`).
fn status_word(st: ExitStatus) -> i32 {
    match st {
        ExitStatus::Exited(rv) => (rv & 0xff) << 8,
        ExitStatus::Signaled(sig) => signo(sig) & 0x7f,
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
    let Some((pid, st)) = kern_wait(p, who, options & WNOHANG != 0)? else { return Ok([0, 0]) };
    let vm = p.vm();
    if status != 0 {
        vm.copyout(status, &status_word(st).to_le_bytes()).map_err(vm_errno)?;
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
        Ok(SelfSignal::Kill(s)) => SyscallResult::Signal(s),
        Ok(_) => SyscallResult::Ok([0, 0]),
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
