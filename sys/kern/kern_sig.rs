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
//! For now only the default action of the signals a fault raises: the
//! process is killed. Which signal a fault raises, and its number, are the
//! ABI's (`SysEntVec::trapsignal`, `SysEntVec::signal_number`).
//!
//! Masks and actions are kept (`sigprocmask`, `sigaction`); delivering a
//! caught signal needs a signal frame, which is the ABI's (`sv_sendsig`).
//!
//! TODO(signal): delivery to handlers, `kill`, pending signals, core dumps.

use sys::errno::{Errno, KResult};
use sys::signal::{SigAction, SigDefault, SigHandler, SigSet, Signal};

use crate::kern_exit::ExitStatus;
use crate::kern_proc::Proc;

/// Log processes killed by signals (`kern.logsigexit`).
const KERN_LOGSIGEXIT: bool = true;

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
            p.abi.signal_number(sig),
            if err.is_some() { "no core dump - " } else { "core dumped" },
            err.unwrap_or("")
        );
    }
    ExitStatus::Signaled(sig)
}

/// Dumps `p`'s core (`coredump`). There is no file system to dump to, so
/// it fails as FreeBSD's does without a dumper (`ENOSYS`).
fn coredump(_p: &Proc) -> Result<(), &'static str> {
    Err("other error")
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
    old
}

/// Sets the action of `sig` if `new` is given, and returns the old one
/// (`kern_sigaction`). `SIGKILL` and `SIGSTOP` keep their default.
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
    }
    Ok(old)
}

/// What sending a signal to oneself does now (`kern_thr_kill` on oneself,
/// `raise`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelfSignal {
    /// Nothing happens (ignored, or its default is to ignore).
    Nothing,
    /// The process is killed by it.
    Kill(Signal),
    /// A handler must run. TODO(signal): delivery.
    Catch,
    /// It is blocked; it would stay pending. TODO(signal): pending signals.
    Blocked,
    /// It stops or continues the process. TODO(signal): job control.
    StopOrContinue,
}

/// Sends `sig` to the calling process and says what follows.
pub fn kern_sigself(p: &Proc, sig: Signal) -> SelfSignal {
    let st = p.sig.lock();
    if st.mask.contains(sig) {
        return SelfSignal::Blocked;
    }
    let action = st.actions.get(sig.index()).copied().unwrap_or_default();
    match action.handler {
        SigHandler::Ignore => SelfSignal::Nothing,
        SigHandler::Catch(_) => SelfSignal::Catch,
        SigHandler::Default => match sig.default_action() {
            SigDefault::Kill | SigDefault::Core => SelfSignal::Kill(sig),
            SigDefault::Ignore => SelfSignal::Nothing,
            SigDefault::Stop | SigDefault::Continue => SelfSignal::StopOrContinue,
        },
    }
}
