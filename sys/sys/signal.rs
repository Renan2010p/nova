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

//! Signals, by name, and the sets and actions that hold them.
//!
//! As with [`crate::errno`], the kernel knows what a signal means but not
//! its number: every ABI in `sys/compat` maps them to its own (`SIGBUS`
//! is 10 for FreeBSD, 7 for some other systems). The list is the union
//! of POSIX and BSD signals; exhaustive, for the same reason as `Errno`.

use libkern::conv::{u64_from_u8, usize_from_u64};

/// A signal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Signal {
    /// Hangup.
    SIGHUP,
    /// Interrupt.
    SIGINT,
    /// Quit.
    SIGQUIT,
    /// Illegal instruction.
    SIGILL,
    /// Trace or breakpoint trap.
    SIGTRAP,
    /// Abort.
    SIGABRT,
    /// Emulator trap.
    SIGEMT,
    /// Arithmetic exception.
    SIGFPE,
    /// Kill (cannot be caught or ignored).
    SIGKILL,
    /// Bus error.
    SIGBUS,
    /// Segmentation violation.
    SIGSEGV,
    /// Bad system call.
    SIGSYS,
    /// Write on a pipe with no reader.
    SIGPIPE,
    /// Alarm clock.
    SIGALRM,
    /// Termination.
    SIGTERM,
    /// Urgent condition on a socket.
    SIGURG,
    /// Stop (cannot be caught or ignored).
    SIGSTOP,
    /// Stop from the terminal.
    SIGTSTP,
    /// Continue a stopped process.
    SIGCONT,
    /// A child stopped or exited.
    SIGCHLD,
    /// Background read from the terminal.
    SIGTTIN,
    /// Background write to the terminal.
    SIGTTOU,
    /// Input/output possible.
    SIGIO,
    /// CPU time limit exceeded.
    SIGXCPU,
    /// File size limit exceeded.
    SIGXFSZ,
    /// Virtual time alarm.
    SIGVTALRM,
    /// Profiling time alarm.
    SIGPROF,
    /// Window size change.
    SIGWINCH,
    /// Information request.
    SIGINFO,
    /// User defined signal 1.
    SIGUSR1,
    /// User defined signal 2.
    SIGUSR2,
    /// Reserved for the thread library.
    SIGTHR,
    /// Reserved for the real-time library.
    SIGLIBRT,
    /// Real-time signal `SIGRTMIN + n`, `n < NSIGRT`.
    Rt(u8),
}

/// Real-time signals (FreeBSD's `SIGRTMAX - SIGRTMIN + 1`).
pub const NSIGRT: u8 = 62;

/// The signals with a name, in index order.
const FIXED: [Signal; 33] = [
        Signal::SIGHUP,
        Signal::SIGINT,
        Signal::SIGQUIT,
        Signal::SIGILL,
        Signal::SIGTRAP,
        Signal::SIGABRT,
        Signal::SIGEMT,
        Signal::SIGFPE,
        Signal::SIGKILL,
        Signal::SIGBUS,
        Signal::SIGSEGV,
        Signal::SIGSYS,
        Signal::SIGPIPE,
        Signal::SIGALRM,
        Signal::SIGTERM,
        Signal::SIGURG,
        Signal::SIGSTOP,
        Signal::SIGTSTP,
        Signal::SIGCONT,
        Signal::SIGCHLD,
        Signal::SIGTTIN,
        Signal::SIGTTOU,
        Signal::SIGIO,
        Signal::SIGXCPU,
        Signal::SIGXFSZ,
        Signal::SIGVTALRM,
        Signal::SIGPROF,
        Signal::SIGWINCH,
        Signal::SIGINFO,
        Signal::SIGUSR1,
        Signal::SIGUSR2,
        Signal::SIGTHR,
        Signal::SIGLIBRT,
];

/// Number of signals the kernel knows: the size of a [`SigSet`].
pub const NSIG: usize = FIXED.len() + usize_from_u64(u64_from_u8(NSIGRT));
const _: () = assert!(NSIG <= 128, "a SigSet is a u128");

/// What happens to a process that does not catch a signal (FreeBSD's
/// `sigproptbl`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SigDefault {
    /// Terminate.
    Kill,
    /// Terminate and dump core.
    Core,
    /// Nothing.
    Ignore,
    /// Stop.
    Stop,
    /// Continue if stopped.
    Continue,
}

impl Signal {
    /// The signal's position in a [`SigSet`], `0..NSIG`.
    pub fn index(self) -> usize {
        match self {
            Signal::Rt(n) => FIXED.len() + usize::from(n),
            s => FIXED.iter().position(|&f| f == s).unwrap_or(0),
        }
    }

    /// The signal at `index`, if any.
    pub fn from_index(index: usize) -> Option<Signal> {
        match FIXED.get(index) {
            Some(&s) => Some(s),
            None => {
                let n = u8::try_from(index.checked_sub(FIXED.len())?).ok()?;
                (n < NSIGRT).then_some(Signal::Rt(n))
            }
        }
    }

    /// The default action.
    pub fn default_action(self) -> SigDefault {
        match self {
            Signal::SIGHUP => SigDefault::Kill,
            Signal::SIGINT => SigDefault::Kill,
            Signal::SIGQUIT => SigDefault::Core,
            Signal::SIGILL => SigDefault::Core,
            Signal::SIGTRAP => SigDefault::Core,
            Signal::SIGABRT => SigDefault::Core,
            Signal::SIGEMT => SigDefault::Core,
            Signal::SIGFPE => SigDefault::Core,
            Signal::SIGKILL => SigDefault::Kill,
            Signal::SIGBUS => SigDefault::Core,
            Signal::SIGSEGV => SigDefault::Core,
            Signal::SIGSYS => SigDefault::Core,
            Signal::SIGPIPE => SigDefault::Kill,
            Signal::SIGALRM => SigDefault::Kill,
            Signal::SIGTERM => SigDefault::Kill,
            Signal::SIGURG => SigDefault::Ignore,
            Signal::SIGSTOP => SigDefault::Stop,
            Signal::SIGTSTP => SigDefault::Stop,
            Signal::SIGCONT => SigDefault::Continue,
            Signal::SIGCHLD => SigDefault::Ignore,
            Signal::SIGTTIN => SigDefault::Stop,
            Signal::SIGTTOU => SigDefault::Stop,
            Signal::SIGIO => SigDefault::Ignore,
            Signal::SIGXCPU => SigDefault::Core,
            Signal::SIGXFSZ => SigDefault::Core,
            Signal::SIGVTALRM => SigDefault::Kill,
            Signal::SIGPROF => SigDefault::Kill,
            Signal::SIGWINCH => SigDefault::Ignore,
            Signal::SIGINFO => SigDefault::Ignore,
            Signal::SIGUSR1 => SigDefault::Kill,
            Signal::SIGUSR2 => SigDefault::Kill,
            Signal::SIGTHR => SigDefault::Kill,
            Signal::SIGLIBRT => SigDefault::Kill,
            Signal::Rt(_) => SigDefault::Kill,
        }
    }

    /// True if the default action dumps core (`SIGPROP_CORE`).
    pub fn dumps_core(self) -> bool {
        self.default_action() == SigDefault::Core
    }

    /// True for the signals that cannot be caught, ignored or blocked.
    pub fn is_uncatchable(self) -> bool {
        matches!(self, Signal::SIGKILL | Signal::SIGSTOP)
    }
}

/// A set of signals (`sigset_t`, by name).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SigSet(u128);

impl SigAction {
    /// The default action, nothing blocked, no option.
    pub const DEFAULT: SigAction = SigAction {
        handler: SigHandler::Default,
        mask: SigSet::EMPTY,
        flags: SigFlags {
            onstack: false,
            restart: false,
            resethand: false,
            nodefer: false,
            nocldwait: false,
            siginfo: false,
            nocldstop: false,
        },
    };
}

impl SigSet {
    /// No signal.
    pub const EMPTY: SigSet = SigSet(0);

    /// True if `sig` is in the set.
    pub fn contains(self, sig: Signal) -> bool {
        self.0 & (1 << sig.index()) != 0
    }

    /// Adds `sig`.
    pub fn insert(&mut self, sig: Signal) {
        self.0 |= 1 << sig.index();
    }

    /// Removes `sig`.
    pub fn remove(&mut self, sig: Signal) {
        self.0 &= !(1 << sig.index());
    }

    /// The signals in either set.
    pub fn union(self, other: SigSet) -> SigSet {
        SigSet(self.0 | other.0)
    }

    /// The signals in `self` but not in `other`.
    pub fn difference(self, other: SigSet) -> SigSet {
        SigSet(self.0 & !other.0)
    }

    /// Every signal in the set.
    pub fn iter(self) -> impl Iterator<Item = Signal> {
        (0..NSIG).filter(move |&i| self.0 & (1 << i) != 0).filter_map(Signal::from_index)
    }

    /// The set without the signals that cannot be blocked.
    pub fn blockable(mut self) -> SigSet {
        self.remove(Signal::SIGKILL);
        self.remove(Signal::SIGSTOP);
        self
    }
}

/// How a signal is handled (`sa_handler`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SigHandler {
    /// The default action (`SIG_DFL`).
    #[default]
    Default,
    /// Ignored (`SIG_IGN`).
    Ignore,
    /// Caught by the user function at this address.
    Catch(u64),
}

/// Options of a signal action (`sa_flags`, by name).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools, reason = "independent POSIX flags")]
pub struct SigFlags {
    /// Run the handler on the alternate stack (`SA_ONSTACK`).
    pub onstack: bool,
    /// Restart interrupted system calls (`SA_RESTART`).
    pub restart: bool,
    /// Reset to the default action when caught (`SA_RESETHAND`).
    pub resethand: bool,
    /// Do not block the signal in its handler (`SA_NODEFER`).
    pub nodefer: bool,
    /// Do not keep zombie children (`SA_NOCLDWAIT`).
    pub nocldwait: bool,
    /// Pass `siginfo` to the handler (`SA_SIGINFO`).
    pub siginfo: bool,
    /// No `SIGCHLD` when a child stops (`SA_NOCLDSTOP`).
    pub nocldstop: bool,
}

/// A signal action (`struct sigaction`, by name).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SigAction {
    /// The handler.
    pub handler: SigHandler,
    /// Signals blocked while the handler runs.
    pub mask: SigSet,
    /// Options.
    pub flags: SigFlags,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_round_trips() {
        for i in 0..NSIG {
            let s = Signal::from_index(i).unwrap();
            assert_eq!(s.index(), i);
        }
        assert!(Signal::from_index(NSIG).is_none());
    }

    #[test]
    fn sets() {
        let mut s = SigSet::EMPTY;
        s.insert(Signal::SIGINT);
        s.insert(Signal::Rt(3));
        s.insert(Signal::SIGKILL);
        assert!(s.contains(Signal::SIGINT) && s.contains(Signal::Rt(3)));
        assert!(!s.blockable().contains(Signal::SIGKILL));
        assert_eq!(s.iter().count(), 3);
    }
}
