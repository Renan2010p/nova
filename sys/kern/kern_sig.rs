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


//! Signals (FreeBSD's `kern/kern_sig.c` and `kern/kern_ucoredump.c`).
//!
//! A signal sent ([`psignal`]) is pending until the process returns to
//! user mode ([`postsig`]): then, unless blocked, its action is taken:
//! ignored, the default (killed, stopped, continued, nothing), or a
//! handler run, through a frame the ABI builds (`SysEntVec::sendsig`) and
//! takes down (`sigreturn`, in the ABI). A sleep a signal may interrupt
//! ends at once, with `EINTR` or `ERESTART`. Which signal a fault
//! raises, and its number, are the ABI's.
//!
//! TODO(signal): `siginfo` of senders, queued real-time signals, the
//! alternate stack, core dumps, threads.

use core::sync::atomic::Ordering;

use framework::sync::WaitQueue;
use framework::user::UserRegs;
use sys::errno::{Errno, KResult};
use sys::signal::{SigAction, SigDefault, SigHandler, SigSet, Signal};

use crate::kern_exit::ExitStatus;
use crate::kern_proc::{PID_INIT, Proc, StopEvent, foreach_proc, pfind, wakeup_waiters};

/// Log processes killed by signals (`kern.logsigexit`).
const KERN_LOGSIGEXIT: bool = true;

/// Stopped processes sleep here until continued (or killed).
static STOPPED: WaitQueue = WaitQueue::new();

/// Kills `p` with `sig`, its default action (`sigexit`).
pub fn sigexit(p: &Proc, sig: Signal) -> ExitStatus {
    if sig.dumps_core() && KERN_LOGSIGEXIT {
        let err = coredump(p).err();
        crate::log!(
            "pid {} ({}), jid {}, uid {}: exited on signal {} ({}{})\n",
            p.pid,
            p.comm(),
            p.jid,
            p.uid,
            p.abi().signals().map_or(0, |s| s.signal_number(sig)),
            if err.is_some() { "no core dump - " } else { "core dumped" },
            err.unwrap_or("")
        );
    }
    ExitStatus::Signaled(sig)
}

/// Dumps `p`'s core (`coredump`). Nova does not write core files yet, so
/// it fails as FreeBSD's does without a dumper.
fn coredump(_p: &Proc) -> Result<(), &'static str> {
    Err("other error")
}

fn is_stop(sig: Signal) -> bool {
    sig.default_action() == SigDefault::Stop
}

/// Sends `sig` to `q` (`tdsendsignal`): it is pending until `q` takes it,
/// unless ignored. `SIGCONT` continues a stopped process at once; a stop
/// signal cancels a pending `SIGCONT`, and the reverse.
pub fn psignal(q: &Proc, sig: Signal) {
    if q.exit_status().is_some() {
        return;
    }
    let mut st = q.sig.lock();
    let action = st.action(sig);
    if sig == Signal::SIGCONT {
        for s in st.pending.iter().filter(|&s| is_stop(s)) {
            st.pending.remove(s);
        }
        if q.stopped.swap(false, Ordering::AcqRel) {
            *q.wait_event.lock() = Some(StopEvent::Continued);
            STOPPED.wake_all();
            wakeup_waiters();
        }
    } else if is_stop(sig) {
        st.pending.remove(Signal::SIGCONT);
    }
    let discard = match action.handler {
        SigHandler::Ignore => true,
        SigHandler::Default => matches!(sig.default_action(), SigDefault::Ignore | SigDefault::Continue),
        SigHandler::Catch(_) => false,
    };
    if discard {
        q.sig_update(&st);
        return;
    }
    st.pending.insert(sig);
    q.sig_update(&st);
    let wake = q.sig_pending() || sig == Signal::SIGKILL;
    drop(st);
    if sig == Signal::SIGKILL && q.stopped.swap(false, Ordering::AcqRel) {
        STOPPED.wake_all();
    }
    if wake {
        let chan = q.wchan.lock().clone();
        if let Some(c) = chan {
            c.wake();
        }
    }
}

/// Why a sleep a signal ended fails (`sleepq_catch_signals`): `ERESTART`
/// if the call is to be restarted once the signal is taken (its handler
/// has `SA_RESTART`, or it stops the process), `EINTR` otherwise.
pub fn sig_interrupt(p: &Proc) -> Errno {
    let st = p.sig.lock();
    let Some(sig) = st.deliverable().iter().next() else { return Errno::EINTR };
    let a = st.action(sig);
    match a.handler {
        SigHandler::Catch(_) if a.flags.restart => Errno::ERESTART,
        SigHandler::Default if is_stop(sig) => Errno::ERESTART,
        _ => Errno::EINTR,
    }
}

/// Stops `p`, on its own thread, until it is continued or killed
/// (`thread_suspend_switch`); its parent learns of it from `wait` and
/// `SIGCHLD`.
fn stop(p: &Proc, sig: Signal) {
    p.stopped.store(true, Ordering::Release);
    *p.wait_event.lock() = Some(StopEvent::Stopped(sig));
    wakeup_waiters();
    if let Some(parent) = pfind(p.ppid()) {
        let nocldstop = parent.sig.lock().action(Signal::SIGCHLD).flags.nocldstop;
        if !nocldstop {
            psignal(&parent, Signal::SIGCHLD);
        }
    }
    STOPPED.wait_until(|| !p.stopped.load(Ordering::Acquire));
}

/// Takes the signals pending for `p` and not blocked (`ast` and
/// `postsig`), on its way back to user mode with `regs`: each is ignored,
/// stops or kills the process, or has its handler called. Returns how
/// the process ends, if a signal kills it.
pub fn postsig(p: &Proc, regs: &mut UserRegs) -> Option<ExitStatus> {
    loop {
        let (sig, action, oldmask) = {
            let mut st = p.sig.lock();
            let Some(sig) = st.deliverable().iter().next() else {
                p.sig_update(&st);
                return None;
            };
            st.pending.remove(sig);
            let action = st.action(sig);
            let oldmask = st.mask;
            if let SigHandler::Catch(_) = action.handler {
                // Blocked while the handler runs: its mask, and itself.
                let mut mask = st.mask.union(action.mask);
                if !action.flags.nodefer {
                    mask.insert(sig);
                }
                st.mask = mask.blockable();
                if action.flags.resethand
                    && let Some(a) = st.actions.get_mut(sig.index())
                {
                    *a = SigAction::DEFAULT;
                }
            }
            p.sig_update(&st);
            (sig, action, oldmask)
        };
        match action.handler {
            SigHandler::Ignore => {}
            SigHandler::Default => match sig.default_action() {
                SigDefault::Kill | SigDefault::Core => return Some(sigexit(p, sig)),
                SigDefault::Stop => stop(p, sig),
                SigDefault::Ignore | SigDefault::Continue => {}
            },
            SigHandler::Catch(handler) => {
                // A frame the program cannot take: it dies (`sigexit`
                // with `SIGILL`, as FreeBSD's `sendsig`). An ABI with no
                // signal model cannot take one either.
                let delivered = match p.abi().signals() {
                    Some(signals) => signals.sendsig(p, sig, handler, &action, oldmask, regs).is_ok(),
                    None => false,
                };
                if !delivered {
                    crate::printf!("pid {} comm {} has trashed its stack, killing\n", p.pid, p.comm());
                    return Some(sigexit(p, Signal::SIGILL));
                }
            }
        }
        if p.sig.lock().pending.contains(Signal::SIGKILL) {
            return Some(sigexit(p, Signal::SIGKILL));
        }
    }
}

/// How `sigprocmask` changes the mask.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SigHow {
    /// Add the set (`SIG_BLOCK`).
    Block,
    /// Remove the set (`SIG_UNBLOCK`).
    Unblock,
    /// Replace the mask (`SIG_SETMASK`).
    SetMask,
}

/// Changes the signal mask and returns the old one (`kern_sigprocmask`).
/// `SIGKILL` and `SIGSTOP` are never blocked.
pub fn kern_sigprocmask(p: &Proc, how: SigHow, set: Option<SigSet>) -> SigSet {
    let mut st = p.sig.lock();
    let old = st.mask;
    if let Some(set) = set {
        st.mask = match how {
            SigHow::Block => old.union(set),
            SigHow::Unblock => old.difference(set),
            SigHow::SetMask => set,
        }
        .blockable();
    }
    p.sig_update(&st);
    old
}

/// Sets the action of `sig` if `new` is given, and returns the old one
/// (`kern_sigaction`). `SIGKILL` and `SIGSTOP` keep their default. A
/// signal now ignored is no longer pending.
pub fn kern_sigaction(p: &Proc, sig: Signal, new: Option<SigAction>) -> KResult<SigAction> {
    if sig.is_uncatchable() && new.is_some_and(|a| a.handler != SigHandler::Default) {
        return Err(Errno::EINVAL);
    }
    let mut st = p.sig.lock();
    let slot = st.actions.get_mut(sig.index()).ok_or(Errno::EINVAL)?;
    let old = *slot;
    if let Some(mut a) = new {
        a.mask = a.mask.blockable();
        *slot = a;
        let ignored = match a.handler {
            SigHandler::Ignore => true,
            SigHandler::Default => sig.default_action() == SigDefault::Ignore,
            SigHandler::Catch(_) => false,
        };
        if ignored {
            st.pending.remove(sig);
        }
    }
    p.sig_update(&st);
    Ok(old)
}

/// The signals pending for `p` (`sigpending`).
pub fn kern_sigpending(p: &Proc) -> SigSet {
    p.sig.lock().pending
}

/// Waits for a signal with `mask` as the mask (`kern_sigsuspend`): always
/// `EINTR` (the handler runs with the mask from before, which the ABI's
/// frame keeps; returns the mask to put back).
pub fn kern_sigsuspend(p: &Proc, mask: SigSet) -> SigSet {
    let old = kern_sigprocmask(p, SigHow::SetMask, Some(mask));
    let never = || false;
    let _ = crate::kern_synch::sleep_sig(p, crate::kern_synch::WChan::Static(&STOPPED), never);
    old
}

/// Which processes `kill` sends to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KillWho {
    /// This process.
    Pid(u32),
    /// The caller's process group.
    OwnGroup,
    /// This process group.
    Group(u32),
    /// Every process but init and the caller (`kill(-1, ...)`).
    All,
}

/// Sends `sig` (`None`: only check) to `who` (`kern_kill`); `ESRCH` if no
/// process matched. As root, every process may be signalled.
pub fn kern_kill(p: &Proc, who: KillWho, sig: Option<Signal>) -> KResult<()> {
    let own = p.pgid();
    let mut targets = 0;
    foreach_proc(|q| {
        let hit = match who {
            KillWho::Pid(pid) => q.pid == pid,
            KillWho::OwnGroup => q.pgid() == own,
            KillWho::Group(g) => q.pgid() == g,
            KillWho::All => q.pid != PID_INIT && q.pid != p.pid,
        };
        if !hit || q.exit_status().is_some() {
            return;
        }
        targets += 1;
        if let Some(sig) = sig {
            psignal(q, sig);
        }
    });
    if targets > 0 { Ok(()) } else { Err(Errno::ESRCH) }
}

/// Sends `sig` to process group `pgid` (`pgsignal`), from the kernel (a
/// terminal's `^C`).
pub fn pgsignal(pgid: u32, sig: Signal) {
    foreach_proc(|q| {
        if q.pgid() == pgid && q.exit_status().is_none() {
            psignal(q, sig);
        }
    });
}
