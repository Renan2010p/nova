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

//! Processes (FreeBSD's `kern/kern_proc.c` and `struct proc`), and the
//! process table (`allproc`).
//!
//! A process is one thread running one image. What `exec` replaces (the
//! address space, the ABI, the command name) sits behind locks; the rest
//! is fixed at creation or atomic.
//!
//! TODO(proc): threads, real credentials, sessions as objects (a
//! controlling terminal), `kinfo_proc`.

use core::fmt;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use framework::mm::{KArc, KBox, KVec, M_WAITOK, MallocType};
use framework::sync::{Mutex, MtxSpin, WaitQueue};
use framework::user::VmSpace;
use sys::errno::{Errno, KResult};
use sys::signal::{NSIG, SigAction, SigHandler, SigSet, Signal};


use crate::kern_abi::SysEntVec;
use crate::kern_descrip::Filedesc;
use crate::kern_exit::ExitStatus;
use crate::vnode::Vnode;

/// Processes (`M_PROC`).
static M_PROC: MallocType = MallocType::new("proc");

/// The pid of `init`.
pub const PID_INIT: u32 = 1;
/// Largest pid (`PID_MAX`); pids wrap around below it.
pub const PID_MAX: u32 = 99_999;

static NEXT_PID: AtomicU32 = AtomicU32::new(PID_INIT);

/// Longest command name (`MAXCOMLEN`).
pub const MAXCOMLEN: usize = framework::task::MAXCOMLEN;

/// A command name (`p_comm`): at most [`MAXCOMLEN`] bytes, valid UTF-8.
#[derive(Clone, Copy)]
pub struct Comm {
    buf: [u8; MAXCOMLEN],
    len: usize,
}

impl Comm {
    /// The last component of `path`, truncated at a character boundary.
    pub fn from_path(path: &str) -> Self {
        let name = path.rsplit('/').next().unwrap_or(path);
        let mut len = name.len().min(MAXCOMLEN);
        while !name.is_char_boundary(len) {
            len -= 1;
        }
        let mut buf = [0u8; MAXCOMLEN];
        if let (Some(dst), Some(src)) = (buf.get_mut(..len), name.as_bytes().get(..len)) {
            dst.copy_from_slice(src);
        }
        Self { buf, len }
    }

    /// The name.
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(self.buf.get(..self.len).unwrap_or(&[])).unwrap_or("?")
    }
}

impl fmt::Debug for Comm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_str(), f)
    }
}

impl fmt::Display for Comm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A process.
pub struct Proc {
    /// Process id (`p_pid`).
    pub pid: u32,
    /// Parent's pid (`p_pptr->p_pid`): 0 for init, whose parent is the
    /// kernel; init's when the parent exits first.
    ppid: AtomicU32,
    /// Process group (`p_pgrp->pg_id`).
    pgid: AtomicU32,
    /// Session (`p_session->s_sid`).
    sid: AtomicU32,
    /// Command name (`p_comm`).
    comm: MtxSpin<Comm>,
    /// The path last executed (`p_textvp`'s name, `kern.proc.pathname`).
    execpath: Mutex<KVec<u8>>,
    /// The arguments it was executed with, each with its NUL
    /// (`p_args`, `kern.proc.args`).
    args: Mutex<KVec<u8>>,
    /// Its thread's id (`td_tid`), once it has one.
    pub tid: AtomicU64,
    /// When it was created, in clock ticks since boot (`p_stats->p_start`).
    pub start_ticks: u64,
    /// Credentials (`p_ucred`).
    cred: MtxSpin<Cred>,
    /// Jail id (`pr_id`): 0, the host.
    pub jid: u32,
    /// The address space (`p_vmspace`).
    vm: MtxSpin<KArc<VmSpace>>,
    /// The ABI the image runs under (`p_sysent`).
    abi: MtxSpin<&'static dyn SysEntVec>,
    /// Signal mask and actions (`td_sigmask`, `p_sigacts`): a few
    /// kilobytes, so on the heap, as FreeBSD's `struct sigacts`.
    pub sig: KBox<MtxSpin<SigState>>,
    /// A signal is pending that is not blocked (`SIGPENDING`): sleeps
    /// that a signal interrupts check it. Kept by [`SigState`]'s users
    /// through [`Proc::sig_update`].
    sig_hint: AtomicBool,
    /// What the process sleeps on, if a signal may interrupt the sleep
    /// (`td_wchan`): a signal sent to it wakes the channel.
    pub wchan: MtxSpin<Option<crate::kern_synch::WChan>>,
    /// Stopped by a signal (`P_STOPPED_SIG`).
    pub stopped: AtomicBool,
    /// A stop or continue its parent has not been told of by `wait` yet
    /// (`P_WAITED` clear).
    pub wait_event: MtxSpin<Option<StopEvent>>,
    /// Open files (`p_fd`).
    pub fd: Filedesc,
    /// Current directory (`pwd_cdir`); `None` without a root file system.
    pub cwd: MtxSpin<Option<Vnode>>,
    /// File creation mask (`pwd_cmask`).
    pub umask: AtomicU32,
    /// Resource limits (`p_limit`).
    pub rlimits: MtxSpin<[crate::kern_resource::Rlimit; crate::kern_resource::NRESOURCES]>,
    /// Nice value (`p_nice`).
    pub nice: core::sync::atomic::AtomicI32,
    /// The real-time interval timer (`p_realtimer`).
    pub itimer: MtxSpin<crate::kern_resource::Itimer>,
    /// Login class (`cr_loginclass`), as `setloginclass` set it.
    pub loginclass: MtxSpin<Comm>,
    /// How it ended, once it has: a zombie (`p_xexit`, `PRS_ZOMBIE`).
    exit: MtxSpin<Option<ExitStatus>>,
    /// Created by `pdfork`: reached through a descriptor, not its
    /// parent's `wait` for any child, and no `SIGCHLD` (`P2_PROCDESC`).
    pub procdesc: AtomicBool,
    /// Reaped as soon as it ends (its descriptor is closed).
    autoreap: AtomicBool,
}

/// A change of a process's state that `wait` reports, besides its end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopEvent {
    /// Stopped by this signal (`WUNTRACED`).
    Stopped(Signal),
    /// Continued (`WCONTINUED`).
    Continued,
}

/// Most supplementary groups (`NGROUPS`; FreeBSD allows more).
pub const NGROUPS: usize = 64;

/// Who a process acts as (`struct ucred`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cred {
    /// Real user id.
    pub ruid: u32,
    /// Effective user id.
    pub euid: u32,
    /// Saved user id.
    pub svuid: u32,
    /// Real group id.
    pub rgid: u32,
    /// Effective group id.
    pub egid: u32,
    /// Saved group id.
    pub svgid: u32,
    /// Supplementary groups in use.
    pub ngroups: usize,
    /// Supplementary groups.
    pub groups: [u32; NGROUPS],
}

impl Cred {
    /// Root, in group wheel (`proc0`'s credentials).
    pub const ROOT: Cred = Cred { ruid: 0, euid: 0, svuid: 0, rgid: 0, egid: 0, svgid: 0, ngroups: 1, groups: [0; NGROUPS] };

    /// The supplementary groups.
    pub fn groups(&self) -> &[u32] {
        self.groups.get(..self.ngroups).unwrap_or(&[])
    }
}

impl Default for Cred {
    fn default() -> Self {
        Self::ROOT
    }
}

/// A process's signal state. One thread per process for now, so the mask
/// is the process's.
#[derive(Clone, Copy, Debug)]
pub struct SigState {
    /// Blocked signals (`td_sigmask`).
    pub mask: SigSet,
    /// What each signal does, by `Signal::index` (`ps_sigact`).
    pub actions: [SigAction; NSIG],
    /// Sent, not yet delivered (`td_siglist`).
    pub pending: SigSet,
}

impl SigState {
    /// Nothing blocked, every action the default (a new image).
    pub const fn new() -> Self {
        Self { mask: SigSet::EMPTY, actions: [SigAction::DEFAULT; NSIG], pending: SigSet::EMPTY }
    }

    /// After `exec`: caught signals go back to their default (`execsigs`);
    /// ignored ones stay ignored, and the mask is kept.
    pub fn exec(&mut self) {
        for a in self.actions.iter_mut() {
            if matches!(a.handler, SigHandler::Catch(_)) {
                *a = SigAction::DEFAULT;
            }
        }
    }

    /// The action for `sig`; `SIGKILL` and `SIGSTOP` always the default.
    pub fn action(&self, sig: Signal) -> SigAction {
        if sig.is_uncatchable() {
            return SigAction::DEFAULT;
        }
        self.actions.get(sig.index()).copied().unwrap_or_default()
    }

    /// Pending signals not blocked.
    pub fn deliverable(&self) -> SigSet {
        self.pending.difference(self.mask.blockable())
    }
}

impl Default for SigState {
    fn default() -> Self {
        Self::new()
    }
}

/// `path` in kernel memory, as kept in a process.
pub fn path_vec(path: &[u8]) -> KResult<KVec<u8>> {
    let mut v = KVec::with_capacity(path.len(), &M_PROC, M_WAITOK)?;
    v.extend_from_slice(path, M_WAITOK)?;
    Ok(v)
}

/// The next free pid (`fork_findpid`): counting up, wrapping below
/// [`PID_MAX`], skipping pids in use as a process, group or session.
fn next_pid() -> u32 {
    loop {
        let pid = NEXT_PID.fetch_add(1, Ordering::Relaxed);
        if pid > PID_MAX {
            let _ = NEXT_PID.compare_exchange(pid + 1, 2, Ordering::Relaxed, Ordering::Relaxed);
            continue;
        }
        let used = ALLPROC.lock().iter().any(|q| q.pid == pid || q.pgid() == pid || q.sid() == pid);
        if !used {
            return pid;
        }
    }
}

impl Proc {
    /// A new process for an image just executed (init), with the next
    /// pid, as root, with the console as descriptors 0, 1 and 2, in the
    /// root directory, its own group and session leader.
    pub fn new(path: &str, vm: KArc<VmSpace>, abi: &'static dyn SysEntVec) -> KResult<Self> {
        let pid = next_pid();
        Ok(Self {
            pid,
            ppid: AtomicU32::new(0),
            pgid: AtomicU32::new(pid),
            sid: AtomicU32::new(pid),
            comm: MtxSpin::new(Comm::from_path(path)),
            execpath: Mutex::new(path_vec(path.as_bytes())?),
            args: Mutex::new(KVec::new(&M_PROC)),
            tid: AtomicU64::new(0),
            start_ticks: crate::kern_clock::ticks(),
            cred: MtxSpin::new(Cred::ROOT),
            jid: 0,
            vm: MtxSpin::new(vm),
            abi: MtxSpin::new(abi),
            sig: KBox::new(MtxSpin::new(SigState::new()), &M_PROC, M_WAITOK)?,
            sig_hint: AtomicBool::new(false),
            wchan: MtxSpin::new(None),
            stopped: AtomicBool::new(false),
            wait_event: MtxSpin::new(None),
            fd: Filedesc::console()?,
            cwd: MtxSpin::new(crate::vfs_mount::rootvnode()),
            umask: AtomicU32::new(0o022),
            rlimits: MtxSpin::new(crate::policy::policy().default_limits()),
            nice: core::sync::atomic::AtomicI32::new(0),
            itimer: MtxSpin::new(crate::kern_resource::Itimer { deadline: 0, interval: 0 }),
            loginclass: MtxSpin::new(Comm::from_path("default")),
            exit: MtxSpin::new(None),
            procdesc: AtomicBool::new(false),
            autoreap: AtomicBool::new(false),
        })
    }

    /// A child of `self` with address space `vm` (the copy `fork1` makes):
    /// the same files, directory, signal actions and mask, group and
    /// session.
    pub fn fork(&self, vm: KArc<VmSpace>) -> KResult<Self> {
        let sig = KBox::new(MtxSpin::new(SigState::new()), &M_PROC, M_WAITOK)?;
        *sig.lock() = *self.sig.lock();
        // A child starts with no signal pending (`fork1`).
        sig.lock().pending = SigSet::EMPTY;
        Ok(Self {
            pid: next_pid(),
            ppid: AtomicU32::new(self.pid),
            pgid: AtomicU32::new(self.pgid()),
            sid: AtomicU32::new(self.sid()),
            comm: MtxSpin::new(self.comm()),
            execpath: Mutex::new(path_vec(&self.execpath.lock())?),
            args: Mutex::new(path_vec(&self.args.lock())?),
            tid: AtomicU64::new(0),
            start_ticks: crate::kern_clock::ticks(),
            cred: MtxSpin::new(self.cred()),
            jid: self.jid,
            vm: MtxSpin::new(vm),
            abi: MtxSpin::new(self.abi()),
            sig,
            sig_hint: AtomicBool::new(false),
            wchan: MtxSpin::new(None),
            stopped: AtomicBool::new(false),
            wait_event: MtxSpin::new(None),
            fd: self.fd.fork()?,
            cwd: MtxSpin::new(*self.cwd.lock()),
            umask: AtomicU32::new(self.umask.load(Ordering::Relaxed)),
            rlimits: MtxSpin::new(*self.rlimits.lock()),
            nice: core::sync::atomic::AtomicI32::new(self.nice.load(Ordering::Relaxed)),
            // A child has no timer running (`fork1`).
            itimer: MtxSpin::new(crate::kern_resource::Itimer { deadline: 0, interval: 0 }),
            loginclass: MtxSpin::new(*self.loginclass.lock()),
            exit: MtxSpin::new(None),
            procdesc: AtomicBool::new(false),
            autoreap: AtomicBool::new(false),
        })
    }

    /// Credentials (`td_ucred`), as they are now.
    pub fn cred(&self) -> Cred {
        *self.cred.lock()
    }

    /// Changes the credentials with `f` (`proc_set_cred`); its error
    /// leaves them as they were.
    pub fn set_cred(&self, f: impl FnOnce(&mut Cred) -> KResult<()>) -> KResult<()> {
        let mut c = self.cred.lock();
        let mut new = *c;
        f(&mut new)?;
        *c = new;
        Ok(())
    }

    /// Effective user id (`cr_uid`), as in log messages.
    pub fn uid(&self) -> u32 {
        self.cred.lock().euid
    }

    /// Command name (`p_comm`).
    pub fn comm(&self) -> Comm {
        *self.comm.lock()
    }

    /// Sets the command name and the path executed, at `exec`.
    pub fn set_comm(&self, path: &str, execpath: KVec<u8>) {
        *self.comm.lock() = Comm::from_path(path);
        *self.execpath.lock() = execpath;
    }

    /// Keeps the arguments of the image being executed (at most 4 KiB, as
    /// `kern.ps_arg_cache_limit` does FreeBSD's).
    pub fn set_args(&self, argv: &[&str]) -> KResult<()> {
        let mut v = KVec::new(&M_PROC);
        for a in argv {
            if v.len() + a.len() + 1 > 4096 {
                break;
            }
            v.extend_from_slice(a.as_bytes(), M_WAITOK)?;
            v.push(0, M_WAITOK)?;
        }
        *self.args.lock() = v;
        Ok(())
    }

    /// Calls `f` with the arguments, each NUL-terminated.
    pub fn with_args<R>(&self, f: impl FnOnce(&[u8]) -> R) -> R {
        f(&self.args.lock())
    }

    /// Calls `f` with the path last executed.
    pub fn with_execpath<R>(&self, f: impl FnOnce(&[u8]) -> R) -> R {
        f(&self.execpath.lock())
    }

    /// The address space.
    pub fn vm(&self) -> KArc<VmSpace> {
        self.vm.lock().clone()
    }

    /// Replaces the address space (at `exec`, and at exit); returns the
    /// old one, to be dropped by the caller outside the lock.
    pub fn set_vm(&self, vm: KArc<VmSpace>) -> KArc<VmSpace> {
        core::mem::replace(&mut *self.vm.lock(), vm)
    }

    /// The ABI the image runs under.
    pub fn abi(&self) -> &'static dyn SysEntVec {
        *self.abi.lock()
    }

    /// Sets the ABI, at `exec`.
    pub fn set_abi(&self, abi: &'static dyn SysEntVec) {
        *self.abi.lock() = abi;
    }

    /// Parent's pid.
    pub fn ppid(&self) -> u32 {
        self.ppid.load(Ordering::Acquire)
    }

    /// Process group.
    pub fn pgid(&self) -> u32 {
        self.pgid.load(Ordering::Acquire)
    }

    /// Session.
    pub fn sid(&self) -> u32 {
        self.sid.load(Ordering::Acquire)
    }

    /// Recomputes the pending-signal hint from `st` (its signal state,
    /// locked by the caller).
    pub fn sig_update(&self, st: &SigState) {
        self.sig_hint.store(st.deliverable().iter().next().is_some(), Ordering::Release);
    }

    /// A signal is pending that is not blocked.
    pub fn sig_pending(&self) -> bool {
        self.sig_hint.load(Ordering::Acquire)
    }

    /// Reaps it as soon as it ends, or now if it has (its process
    /// descriptor was closed).
    pub fn set_autoreap(&self) {
        self.autoreap.store(true, Ordering::Release);
        if self.exit_status().is_some() {
            drop(proc_unregister(self.pid));
        }
    }

    /// How it ended, if it is a zombie.
    pub fn exit_status(&self) -> Option<ExitStatus> {
        *self.exit.lock()
    }
}

/// Every process not yet reaped (`allproc`), zombies included.
static ALLPROC: Mutex<KVec<KArc<Proc>>> = Mutex::new(KVec::new(&M_PROC));
/// Woken whenever a process exits (the parent sleeps on it in `wait`).
static PROC_EXITED: WaitQueue = WaitQueue::new();
/// Counts exits, so a waiter can tell one happened since it looked.
static EXIT_GEN: AtomicU64 = AtomicU64::new(0);

/// Puts `p` in the table, as a `KArc` its thread and the table share.
pub fn proc_register(p: Proc) -> KResult<KArc<Proc>> {
    let p = KArc::new(p, &M_PROC, M_WAITOK)?;
    ALLPROC.lock().push(p.clone(), M_WAITOK)?;
    Ok(p)
}

/// Takes `pid` out of the table (a failed fork, or a reaped zombie).
fn proc_unregister(pid: u32) -> Option<KArc<Proc>> {
    let mut all = ALLPROC.lock();
    let i = all.iter().position(|q| q.pid == pid)?;
    Some(all.remove(i))
}

/// The process `pid` (`pfind`), zombies included.
pub fn pfind(pid: u32) -> Option<KArc<Proc>> {
    ALLPROC.lock().iter().find(|q| q.pid == pid).cloned()
}

/// Calls `f` on every process (`FOREACH_PROC_IN_SYSTEM`), with the table
/// locked: `f` must not take it again.
pub fn foreach_proc(mut f: impl FnMut(&KArc<Proc>)) {
    for q in ALLPROC.lock().iter() {
        f(q);
    }
}

/// Marks `p` a zombie with `status`, gives its children to init, and
/// wakes its parent (the end of `exit1`).
pub(crate) fn proc_zombie(p: &Proc, status: ExitStatus) {
    for q in ALLPROC.lock().iter().filter(|q| q.ppid() == p.pid) {
        q.ppid.store(PID_INIT, Ordering::Release);
    }
    *p.exit.lock() = Some(status);
    if p.autoreap.load(Ordering::Acquire) {
        drop(proc_unregister(p.pid));
    }
    wakeup_waiters();
}

/// Undoes [`proc_register`] when a fork fails after it.
pub(crate) fn proc_forget(pid: u32) {
    drop(proc_unregister(pid));
}

/// Which children `wait` waits for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaitWho {
    /// Any child (`WAIT_ANY`).
    Any,
    /// This one.
    Pid(u32),
    /// Any in this process group.
    Pgid(u32),
}

/// What `wait` reports, besides ended children.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WaitOptions {
    /// Do not sleep (`WNOHANG`).
    pub nohang: bool,
    /// Stopped children (`WUNTRACED`).
    pub stopped: bool,
    /// Continued children (`WCONTINUED`).
    pub continued: bool,
    /// Leave the child as it is: report it again next time (`WNOWAIT`).
    pub nowait: bool,
}

/// What happened to a child, as `wait` reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaitEvent {
    /// It ended.
    Exited(ExitStatus),
    /// It stopped, or continued.
    Changed(StopEvent),
}

/// Bumps the exit generation and wakes parents in `wait`.
pub(crate) fn wakeup_waiters() {
    EXIT_GEN.fetch_add(1, Ordering::AcqRel);
    PROC_EXITED.wake_all();
}

/// Waits for a child of `p` to end, stop or continue, as `opts` asks
/// (`kern_wait6`): its pid and what happened; an ended child is reaped.
/// `None` if `nohang` and nothing happened yet; `ECHILD` if no child
/// matches; interrupted by a signal, `EINTR` or `ERESTART`.
pub fn kern_wait(p: &Proc, who: WaitWho, opts: WaitOptions) -> KResult<Option<(u32, WaitEvent)>> {
    loop {
        let seen = EXIT_GEN.load(Ordering::Acquire);
        let mut matched = false;
        let mut found = None;
        for q in ALLPROC.lock().iter().filter(|q| q.ppid() == p.pid) {
            // A child with a process descriptor is waited for only by pid.
            let pd = q.procdesc.load(Ordering::Acquire);
            let wanted = match who {
                WaitWho::Any => !pd,
                WaitWho::Pid(pid) => q.pid == pid,
                WaitWho::Pgid(g) => q.pgid() == g && !pd,
            };
            if !wanted {
                continue;
            }
            matched = true;
            if let Some(st) = q.exit_status() {
                found = Some((q.pid, WaitEvent::Exited(st)));
                break;
            }
            let mut ev = q.wait_event.lock();
            let report = match *ev {
                Some(StopEvent::Stopped(_)) => opts.stopped,
                Some(StopEvent::Continued) => opts.continued,
                None => false,
            };
            if let (true, Some(e)) = (report, *ev) {
                if !opts.nowait {
                    *ev = None;
                }
                found = Some((q.pid, WaitEvent::Changed(e)));
                break;
            }
        }
        if let Some((pid, ev)) = found {
            if matches!(ev, WaitEvent::Exited(_)) && !opts.nowait {
                // Reaped: the table's reference goes; the rest went at exit.
                drop(proc_unregister(pid));
            }
            return Ok(Some((pid, ev)));
        }
        if !matched {
            return Err(Errno::ECHILD);
        }
        if opts.nohang {
            return Ok(None);
        }
        crate::kern_synch::sleep_sig(p, crate::kern_synch::WChan::Static(&PROC_EXITED), || {
            EXIT_GEN.load(Ordering::Acquire) != seen
        })?;
    }
}

/// Sets the process group of `pid` (0: `p`) to `pgid` (0: the same as
/// the pid) (`setpgid`). Only `p` and its children, in its session.
pub fn kern_setpgid(p: &Proc, pid: u32, pgid: u32) -> KResult<()> {
    let target = if pid == 0 || pid == p.pid { None } else { Some(pfind(pid).ok_or(Errno::ESRCH)?) };
    let q: &Proc = target.as_deref().unwrap_or(p);
    if q.pid != p.pid && q.ppid() != p.pid {
        return Err(Errno::ESRCH);
    }
    if q.sid() != p.sid() || q.sid() == q.pid {
        return Err(Errno::EPERM);
    }
    let pgid = if pgid == 0 { q.pid } else { pgid };
    if pgid != q.pid {
        // Joining an existing group: it must be in the same session.
        let mut found = false;
        foreach_proc(|r| found |= r.pgid() == pgid && r.sid() == q.sid());
        if !found {
            return Err(Errno::EPERM);
        }
    }
    q.pgid.store(pgid, Ordering::Release);
    Ok(())
}

/// Makes `p` the leader of a new session and group (`setsid`); `EPERM` if
/// it already leads a group.
pub fn kern_setsid(p: &Proc) -> KResult<u32> {
    let mut leads = false;
    foreach_proc(|q| leads |= q.pgid() == p.pid);
    if leads {
        return Err(Errno::EPERM);
    }
    p.pgid.store(p.pid, Ordering::Release);
    p.sid.store(p.pid, Ordering::Release);
    Ok(p.pid)
}

/// The group of `pid` (0: `p`) (`getpgid`).
pub fn kern_getpgid(p: &Proc, pid: u32) -> KResult<u32> {
    if pid == 0 || pid == p.pid {
        return Ok(p.pgid());
    }
    pfind(pid).map(|q| q.pgid()).ok_or(Errno::ESRCH)
}

/// The session of `pid` (0: `p`) (`getsid`).
pub fn kern_getsid(p: &Proc, pid: u32) -> KResult<u32> {
    if pid == 0 || pid == p.pid {
        return Ok(p.sid());
    }
    pfind(pid).map(|q| q.sid()).ok_or(Errno::ESRCH)
}

/// What a process is doing (`ki_stat`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcState {
    /// Running or ready to.
    Run,
    /// Asleep in the kernel.
    Sleep,
    /// Stopped by a signal.
    Stop,
    /// Ended, not yet reaped.
    Zombie,
}

/// A snapshot of a process, for `ps` (`struct kinfo_proc`, by name).
#[derive(Clone, Copy, Debug)]
pub struct ProcInfo {
    /// Process id.
    pub pid: u32,
    /// Parent.
    pub ppid: u32,
    /// Process group.
    pub pgid: u32,
    /// Session.
    pub sid: u32,
    /// Credentials.
    pub cred: Cred,
    /// What it is doing.
    pub state: ProcState,
    /// Nice value.
    pub nice: i32,
    /// Thread id.
    pub tid: u64,
    /// Command name.
    pub comm: Comm,
    /// Login class.
    pub loginclass: Comm,
    /// The controlling terminal's device and foreground group, if its
    /// session has one.
    pub tty: Option<(u64, u32)>,
    /// Address space size and resident bytes.
    pub vsize: u64,
    /// Resident bytes.
    pub rss: u64,
    /// When it started, in clock ticks since boot.
    pub start_ticks: u64,
    /// The ABI it runs under.
    pub abi_name: &'static str,
}

impl Proc {
    /// A snapshot of `self` (`fill_kinfo_proc`).
    pub fn info(&self) -> ProcInfo {
        let state = if self.exit_status().is_some() {
            ProcState::Zombie
        } else if self.stopped.load(Ordering::Acquire) {
            ProcState::Stop
        } else if self.wchan.lock().is_some() {
            ProcState::Sleep
        } else {
            ProcState::Run
        };
        let (vsize, rss) = self.vm().sizes();
        let sid = self.sid();
        let tty = crate::tty::tty_is_ctty(sid).then(|| (crate::tty::tty_ctty_dev(), crate::tty::tty_fg_pgrp()));
        ProcInfo {
            pid: self.pid,
            ppid: self.ppid(),
            pgid: self.pgid(),
            sid,
            cred: self.cred(),
            state,
            nice: self.nice.load(Ordering::Relaxed),
            tid: self.tid.load(Ordering::Relaxed),
            comm: self.comm(),
            loginclass: *self.loginclass.lock(),
            tty,
            vsize,
            rss,
            start_ticks: self.start_ticks,
            abi_name: self.abi().name(),
        }
    }
}
