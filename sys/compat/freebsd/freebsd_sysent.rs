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
use framework::user::Inherit;
use kern::kern_sig::{SelfSignal, kern_sigself};
use vm::vm_mmap::{MmapAt, kern_minherit, kern_mmap_anon, kern_mprotect, kern_munmap};
use kern::sys_generic::{TtyOp, TtyReply, kern_tty_ioctl};
use crate::freebsd_types::{STAT_SIZE, TERMIOS_SIZE, WINSIZE_SIZE, stat_out, termios_in, termios_out, winsize_in, winsize_out};

use crate::arch::sysarch;
use crate::freebsd_sysctl;
use crate::freebsd_signal::{
    SIGACTION_SIZE, SIGSET_SIZE, from_signo, sigaction_in, sigaction_out, sigset_in, sigset_out,
};
use crate::freebsd_syscalls::{
    SYS___getcwd, SYS___sysctl, SYS___sysctlbyname, SYS_close, SYS_fstat, SYS_fstatat, SYS_getegid, SYS_geteuid, SYS_getgid, SYS_getppid, SYS_getuid, SYS_ioctl, SYS_mprotect, SYS_read, SYS__exit, SYS_cpuset_getaffinity, SYS_getrandom, SYS_minherit, SYS_thr_kill, SYS_MAXSYSCALL, SYS_clock_gettime, SYS_getpid, SYS_issetugid, SYS_madvise, SYS_mmap, SYS_munmap,
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
        SYS___sysctlbyname => freebsd_sysctl::sys_sysctlbyname(p, args).into(),
        SYS_getrandom => sys_getrandom(p, args[0], args[1]).into(),
        SYS_minherit => sys_minherit(p, args[0], args[1], int(2)).into(),
        SYS_cpuset_getaffinity => sys_cpuset_getaffinity(p, args[3], args[4]).into(),
        SYS_thr_kill => sys_thr_kill(p, args[0], int(1)),
        SYS_read => sys_generic::kern_read(p, int(0), args[1], args[2]).map(|n| [n, 0]).into(),
        SYS_close => sys_generic::kern_close(p, int(0)).map(|()| [0, 0]).into(),
        SYS_ioctl => sys_ioctl(p, int(0), args[1], args[2]).into(),
        SYS_fstat => sys_fstat(p, int(0), args[1]).into(),
        SYS_fstatat => sys_fstatat(p, int(0), args[1], args[2], int(3)).into(),
        SYS___getcwd => sys_getcwd(p, args[0], args[1]).into(),
        SYS_mprotect => sys_mprotect(p, args[0], args[1], int(2)).into(),
        SYS_getuid => Ok([u64::from(kern_prot::kern_getcred(p).ruid), 0]).into(),
        SYS_geteuid => Ok([u64::from(kern_prot::kern_getcred(p).euid), 0]).into(),
        SYS_getgid => Ok([u64::from(kern_prot::kern_getcred(p).rgid), 0]).into(),
        SYS_getegid => Ok([u64::from(kern_prot::kern_getcred(p).egid), 0]).into(),
        SYS_getppid => Ok([u64::from(kern_prot::kern_getppid(p)), 0]).into(),
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

/// `getrandom(buf, buflen, flags)`: the kernel's generator never blocks
/// once seeded, so the flags change nothing.
fn sys_getrandom(p: &Proc, buf: u64, len: u64) -> KResult<[u64; 2]> {
    let mut chunk = [0u8; 256];
    let mut done = 0u64;
    while done < len {
        let n = usize::try_from((len - done).min(256)).map_err(|_| Errno::EINVAL)?;
        kern::kern_random::arc4random_buf(&mut chunk[..n]);
        p.vm.copyout(buf + done, &chunk[..n]).map_err(vm_errno)?;
        done += u64::try_from(n).map_err(|_| Errno::EINVAL)?;
    }
    Ok([len, 0])
}

/// `minherit(addr, len, inherit)`.
fn sys_minherit(p: &Proc, addr: u64, len: u64, inherit: i32) -> KResult<[u64; 2]> {
    let inherit = match inherit {
        0 => Inherit::Share,
        1 => Inherit::Copy,
        2 => Inherit::None,
        3 => Inherit::Zero,
        _ => return Err(Errno::EINVAL),
    };
    kern_minherit(&p.vm, addr, len, inherit)?;
    Ok([0, 0])
}

/// `cpuset_getaffinity(level, which, id, cpusetsize, mask)`: every CPU,
/// until there are cpusets.
fn sys_cpuset_getaffinity(p: &Proc, size: u64, mask: u64) -> KResult<[u64; 2]> {
    let ncpus = framework::cpu::mp_ncpus();
    let need = u64::from(ncpus.div_ceil(8));
    if size < need || size > 1024 {
        return Err(Errno::ERANGE);
    }
    let mut set = [0u8; 1024];
    for cpu in 0..ncpus {
        let byte = usize_from_u32(cpu / 8);
        set[byte] |= 1 << (cpu % 8);
    }
    p.vm.copyout(mask, &set[..usize::try_from(size).map_err(|_| Errno::EINVAL)?]).map_err(vm_errno)?;
    Ok([0, 0])
}

/// `thr_kill(id, sig)`: only to the calling thread, the only one there is.
fn sys_thr_kill(p: &Proc, id: u64, sig: i32) -> SyscallResult {
    if id != kern_thr::kern_thr_self() {
        return SyscallResult::Err(Errno::ESRCH);
    }
    if sig == 0 {
        return SyscallResult::Ok([0, 0]);
    }
    let Some(sig) = from_signo(sig) else {
        return SyscallResult::Err(Errno::EINVAL);
    };
    match kern_sigself(p, sig) {
        SelfSignal::Kill(sig) => SyscallResult::Signal(sig),
        other => {
            if kern::init_main::bootverbose() && other != SelfSignal::Nothing {
                printf!("pid {} comm {}: signal {:?}: {:?} (not implemented yet)\n", p.pid, p.comm(), sig, other);
            }
            SyscallResult::Ok([0, 0])
        }
    }
}

/// `ioctl(fd, cmd, data)`: the terminal requests (`sys/ttycom.h`, numbers
/// from `tools/abidump`) and the descriptor flags.
fn sys_ioctl(p: &Proc, fd: i32, cmd: u64, data: u64) -> KResult<[u64; 2]> {
    const TIOCGETA: u32 = 0x402c_7413;
    const TIOCSETA: u32 = 0x802c_7414;
    const TIOCSETAW: u32 = 0x802c_7415;
    const TIOCSETAF: u32 = 0x802c_7416;
    const TIOCGWINSZ: u32 = 0x4008_7468;
    const TIOCSWINSZ: u32 = 0x8008_7467;
    const TIOCGPGRP: u32 = 0x4004_7477;
    const TIOCSPGRP: u32 = 0x8004_7476;
    const FIONBIO: u32 = 0x8004_667e;
    const FIOCLEX: u32 = 0x2000_6601;
    const FIONCLEX: u32 = 0x2000_6602;
    // `u_long` on the wire, but the numbers are 32 bits.
    match libkern::conv::trunc_u32(cmd) {
        TIOCGETA => {
            let TtyReply::Attr(t) = kern_tty_ioctl(p, fd, TtyOp::GetAttr)? else { return Err(Errno::ENOTTY) };
            p.vm.copyout(data, &termios_out(&t)).map_err(vm_errno)?;
        }
        c @ (TIOCSETA | TIOCSETAW | TIOCSETAF) => {
            let mut b = [0u8; TERMIOS_SIZE];
            p.vm.copyin(data, &mut b).map_err(vm_errno)?;
            kern_tty_ioctl(p, fd, TtyOp::SetAttr { termios: termios_in(&b), flush: c == TIOCSETAF })?;
        }
        TIOCGWINSZ => {
            let TtyReply::Winsize(w) = kern_tty_ioctl(p, fd, TtyOp::GetWinsize)? else { return Err(Errno::ENOTTY) };
            p.vm.copyout(data, &winsize_out(&w)).map_err(vm_errno)?;
        }
        TIOCSWINSZ => {
            let mut b = [0u8; WINSIZE_SIZE];
            p.vm.copyin(data, &mut b).map_err(vm_errno)?;
            kern_tty_ioctl(p, fd, TtyOp::SetWinsize(winsize_in(&b)))?;
        }
        // No sessions yet, so no controlling terminal: as FreeBSD answers a
        // process that has none (job control is then turned off).
        TIOCGPGRP | TIOCSPGRP => {
            sys_generic::kern_fd_valid(p, fd)?;
            return Err(Errno::ENOTTY);
        }
        // Close-on-exec and non-blocking I/O: nothing to change yet.
        FIONBIO | FIOCLEX | FIONCLEX => sys_generic::kern_fd_valid(p, fd)?,
        _ => {
            sys_generic::kern_fd_valid(p, fd)?;
            return Err(Errno::ENOTTY);
        }
    }
    Ok([0, 0])
}

/// `fstat(fd, sb)`.
fn sys_fstat(p: &Proc, fd: i32, sb: u64) -> KResult<[u64; 2]> {
    let st = sys_generic::kern_fstat(p, fd)?;
    let out: [u8; STAT_SIZE] = stat_out(&st);
    p.vm.copyout(sb, &out).map_err(vm_errno)?;
    Ok([0, 0])
}

/// `fstatat(fd, path, sb, flag)`.
fn sys_fstatat(p: &Proc, fd: i32, path: u64, sb: u64, flag: i32) -> KResult<[u64; 2]> {
    const AT_EMPTY_PATH: i32 = 0x4000;
    let mut first = [0u8; 1];
    p.vm.copyin(path, &mut first).map_err(vm_errno)?;
    let st = vfs_syscalls::kern_statat(p, fd, first[0] == 0, flag & AT_EMPTY_PATH != 0)?;
    p.vm.copyout(sb, &stat_out(&st)).map_err(vm_errno)?;
    Ok([0, 0])
}

/// `__getcwd(buf, size)`: the path, with its NUL.
fn sys_getcwd(p: &Proc, buf: u64, size: u64) -> KResult<[u64; 2]> {
    let cwd = vfs_syscalls::kern_getcwd(p).as_bytes();
    let need = libkern::conv::u64_from_usize(cwd.len() + 1);
    if size < need {
        return Err(Errno::ERANGE);
    }
    p.vm.copyout(buf, cwd).map_err(vm_errno)?;
    p.vm.copyout(buf + need - 1, &[0]).map_err(vm_errno)?;
    Ok([0, 0])
}

/// `mprotect(addr, len, prot)`.
fn sys_mprotect(p: &Proc, addr: u64, len: u64, prot: i32) -> KResult<[u64; 2]> {
    let prot = Prot { read: prot & 7 != 0, write: prot & 2 != 0, exec: prot & 4 != 0 };
    kern_mprotect(&p.vm, addr, len, prot)?;
    Ok([0, 0])
}
