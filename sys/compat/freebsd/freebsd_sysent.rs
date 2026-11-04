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

//! The FreeBSD system call table (`sysent[]`, `kern/init_sysent.c`):
//! decodes each call's arguments from the FreeBSD ABI and calls the
//! kernel's implementation. Numbers and names come from the generated
//! [`crate::freebsd_syscalls`].
//!
//! A number FreeBSD does not have is `nosys`, as in FreeBSD: `SIGSYS`. A
//! call FreeBSD has but Nova does not implement yet fails with `ENOSYS`
//! (a FreeBSD program may handle that), logged on a verbose boot.

use framework::user::{Prot, UserRegs};
use kern::kern_exec::vm_errno;
use kern::kern_proc::{PID_INIT, Proc};
use kern::kern_sig::{SigHow, kern_sigaction, kern_sigprocmask};
use kern::kern_clock::{ClockId, kern_clock_gettime};
use kern::{kern_prot, kern_thr, printf, sys_generic, vfs_syscalls};
use libkern::conv::{trunc_i32, usize_from_u32};
use sys::errno::{Errno, KResult};
use sys::signal::Signal;
use vm::vm_mmap::{MmapAt, kern_mmap_anon, kern_munmap};

use crate::arch::sysarch;
use crate::freebsd_sysctl;
use crate::freebsd_signal::{
    SIGACTION_SIZE, SIGSET_SIZE, from_signo, sigaction_in, sigaction_out, sigset_in, sigset_out,
};
use crate::freebsd_syscalls::{
    SYS___sysctl, SYS__exit, SYS_MAXSYSCALL, SYS_clock_gettime, SYS_getpid, SYS_issetugid, SYS_madvise, SYS_mmap, SYS_munmap,
    SYS_readlink, SYS_sigaction, SYS_sigprocmask, SYS_sysarch, SYS_thr_self, SYS_write, SYSCALLNAMES,
};

/// The outcome of one system call.
pub(crate) enum SyscallResult {
    /// Success, with `td_retval[0..2]`.
    Ok([u64; 2]),
    /// Failure.
    Err(Errno),
    /// `_exit(rval)`.
    Exit(i32),
    /// A signal to the caller.
    Signal(Signal),
}

impl From<KResult<[u64; 2]>> for SyscallResult {
    fn from(r: KResult<[u64; 2]>) -> Self {
        match r {
            Ok(rv) => SyscallResult::Ok(rv),
            Err(e) => SyscallResult::Err(e),
        }
    }
}

/// Runs system call `code` with `args` for `p`, whose registers are
/// `regs` (for the calls that change them, such as `sysarch`).
#[allow(non_upper_case_globals)] // FreeBSD's `SYS_*` names
pub(crate) fn syscall(p: &Proc, regs: &mut UserRegs, code: u64, args: &[u64; 6]) -> SyscallResult {
    let Ok(code) = u32::try_from(code) else {
        return nosys(p, code);
    };
    // `int` arguments are the low 32 bits of their register.
    let int = |i: usize| trunc_i32(args[i]);
    match code {
        SYS__exit => SyscallResult::Exit(int(0)),
        SYS_write => sys_generic::kern_write(p, int(0), args[1], args[2]).map(|n| [n, 0]).into(),
        SYS_getpid => Ok([u64::from(kern_prot::kern_getpid(p)), 0]).into(),
        SYS_issetugid => Ok([u64::from(kern_prot::kern_issetugid(p)), 0]).into(),
        SYS_readlink => vfs_syscalls::kern_readlinkat(p, args[0], args[1], args[2]).map(|n| [n, 0]).into(),
        SYS_thr_self => sys_thr_self(p, args[0]).into(),
        SYS_sigprocmask => sys_sigprocmask(p, int(0), args[1], args[2]).into(),
        SYS_sigaction => sys_sigaction(p, int(0), args[1], args[2]).into(),
        SYS_mmap => sys_mmap(p, args).into(),
        SYS_munmap => kern_munmap(&p.vm, args[0], args[1]).map(|()| [0, 0]).into(),
        // Advice only: ignoring it is allowed.
        SYS_madvise => Ok([0, 0]).into(),
        SYS_sysarch => sysarch(p, regs, int(0), args[1]).into(),
        SYS_clock_gettime => sys_clock_gettime(p, int(0), args[1]).into(),
        SYS___sysctl => freebsd_sysctl::sys_sysctl(p, args).into(),
        c if c >= SYS_MAXSYSCALL || is_nosys(SYSCALLNAMES[usize_from_u32(c)]) => nosys(p, u64::from(c)),
        c => {
            if kern::init_main::bootverbose() {
                printf!(
                    "pid {} comm {}: {} ({}) not implemented\n",
                    p.pid,
                    p.comm(),
                    SYSCALLNAMES[usize_from_u32(c)],
                    c
                );
            }
            SyscallResult::Err(Errno::ENOSYS)
        }
    }
}

/// Slots without a system call: reserved, obsolete, or for loadable ones.
fn is_nosys(name: &str) -> bool {
    name.starts_with('#') || name.starts_with("obs_") || name == "lkmnosys"
}

/// `kern_nosys`: `SIGSYS` (the default `kern.signosys`), logged for init
/// (the default `kern.lognosys`).
fn nosys(p: &Proc, code: u64) -> SyscallResult {
    if p.pid == PID_INIT {
        printf!("pid {} comm {}: nosys {}\n", p.pid, p.comm(), code);
    }
    SyscallResult::Signal(Signal::SIGSYS)
}

/// `thr_self(long *id)`.
fn sys_thr_self(p: &Proc, id: u64) -> KResult<[u64; 2]> {
    p.vm.copyout(id, &kern_thr::kern_thr_self().to_le_bytes()).map_err(vm_errno)?;
    Ok([0, 0])
}

/// `sigprocmask(how, set, oset)`.
fn sys_sigprocmask(p: &Proc, how: i32, set: u64, oset: u64) -> KResult<[u64; 2]> {
    const SIG_BLOCK: i32 = 1;
    const SIG_UNBLOCK: i32 = 2;
    const SIG_SETMASK: i32 = 3;
    let new = if set == 0 {
        None
    } else {
        let mut b = [0u8; SIGSET_SIZE];
        p.vm.copyin(set, &mut b).map_err(vm_errno)?;
        Some(sigset_in(&b))
    };
    let how = match how {
        SIG_BLOCK => SigHow::Block,
        SIG_UNBLOCK => SigHow::Unblock,
        SIG_SETMASK => SigHow::SetMask,
        _ if new.is_none() => SigHow::Block,
        _ => return Err(Errno::EINVAL),
    };
    let old = kern_sigprocmask(p, how, new);
    if oset != 0 {
        p.vm.copyout(oset, &sigset_out(old)).map_err(vm_errno)?;
    }
    Ok([0, 0])
}

/// `sigaction(sig, act, oact)`.
fn sys_sigaction(p: &Proc, sig: i32, act: u64, oact: u64) -> KResult<[u64; 2]> {
    let sig = from_signo(sig).ok_or(Errno::EINVAL)?;
    let new = if act == 0 {
        None
    } else {
        let mut b = [0u8; SIGACTION_SIZE];
        p.vm.copyin(act, &mut b).map_err(vm_errno)?;
        Some(sigaction_in(&b))
    };
    let old = kern_sigaction(p, sig, new)?;
    if oact != 0 {
        p.vm.copyout(oact, &sigaction_out(&old)).map_err(vm_errno)?;
    }
    Ok([0, 0])
}

/// `mmap(addr, len, prot, flags, fd, pos)`: anonymous memory only, until
/// there are files.
fn sys_mmap(p: &Proc, args: &[u64; 6]) -> KResult<[u64; 2]> {
    const PROT_READ: i32 = 0x1;
    const PROT_WRITE: i32 = 0x2;
    const PROT_EXEC: i32 = 0x4;
    const MAP_FIXED: i32 = 0x0010;
    const MAP_ANON: i32 = 0x1000;
    const MAP_GUARD: i32 = 0x2000;
    const MAP_EXCL: i32 = 0x4000;
    const MAP_32BIT: i32 = 0x0008_0000;
    const MAP_ALIGNMENT_SHIFT: u32 = 24;
    const MAP_ALIGNED_SUPER: u32 = 1;
    let (addr, len) = (args[0], args[1]);
    let (prot, flags, fd) = (trunc_i32(args[2]), trunc_i32(args[3]), trunc_i32(args[4]));
    if flags & (MAP_ANON | MAP_GUARD) == 0 {
        // A file mapping: there are no files yet, so no valid descriptor.
        return Err(Errno::EBADF);
    }
    if fd != -1 || args[5] != 0 || flags & MAP_32BIT != 0 {
        return Err(Errno::EINVAL);
    }
    if flags & MAP_GUARD != 0 && prot != 0 {
        return Err(Errno::EINVAL);
    }
    let prot = Prot {
        read: prot & (PROT_READ | PROT_WRITE | PROT_EXEC) != 0,
        write: prot & PROT_WRITE != 0,
        exec: prot & PROT_EXEC != 0,
    };
    let align = u32::from_le_bytes(flags.to_le_bytes()) >> MAP_ALIGNMENT_SHIFT;
    let align_shift = match align {
        0 => 0,
        MAP_ALIGNED_SUPER => 21, // a 2 MiB superpage
        n if (12..64).contains(&n) => n,
        _ => return Err(Errno::EINVAL),
    };
    let at = match (flags & MAP_FIXED != 0, flags & MAP_EXCL != 0) {
        (true, false) => MmapAt::Fixed(addr),
        (true, true) => MmapAt::FixedNoReplace(addr),
        (false, _) => MmapAt::Hint { addr, align_shift },
    };
    // TODO(fork): MAP_SHARED anonymous memory is private until there are
    // other processes to share it with.
    Ok([kern_mmap_anon(&p.vm, at, len, prot)?, 0])
}

/// `clock_gettime(clock_id, tp)`.
fn sys_clock_gettime(p: &Proc, id: i32, tp: u64) -> KResult<[u64; 2]> {
    // FreeBSD's `clockid_t` values (`sys/_clock_id.h`).
    let clock = match id {
        0 | 9 | 10 => ClockId::Realtime,           // REALTIME, _PRECISE, _FAST
        4 | 5 | 7 | 8 | 11 | 12 => ClockId::Monotonic, // MONOTONIC, UPTIME and variants
        13 => ClockId::Second,
        _ => return Err(Errno::EINVAL), // CPU-time clocks: TODO(rusage)
    };
    let ts = kern_clock_gettime(clock);
    let mut b = [0u8; 16];
    b[..8].copy_from_slice(&ts.sec.to_le_bytes());
    b[8..].copy_from_slice(&ts.nsec.to_le_bytes());
    p.vm.copyout(tp, &b).map_err(vm_errno)?;
    Ok([0, 0])
}
