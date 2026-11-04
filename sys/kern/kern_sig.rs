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
//! TODO(signal): handlers, masks, `kill`, signal frames (the ABI's
//! `sv_sendsig`), core dumps.

use sys::signal::Signal;

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
            p.comm,
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
