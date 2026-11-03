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

//! Panic and halt (FreeBSD's `kern/kern_shutdown.c`).
//!
//! Every Rust `panic!` in the kernel, MD or MI, ends up in [`panic`]. MD
//! code can call `panic!` without depending on this crate: the handler is
//! bound at link time by the language.

use core::panic::PanicInfo;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::subr_prf::vprintf_unlocked;

static PANICKING: AtomicBool = AtomicBool::new(false);

/// The kernel panic handler: print the message and stop this CPU.
#[cfg(not(test))]
#[panic_handler]
fn panic(info: &PanicInfo<'_>) -> ! {
    // Never dropped: we stay on this CPU with interrupts off until halt.
    let _intr = framework::cpu::intr_guard();
    if PANICKING.swap(true, Ordering::AcqRel) {
        // Another CPU is panicking, or printing is what failed: just stop.
        // TODO(smp): once APs run, the first panicking CPU stops the
        // others with an IPI (FreeBSD's stop_cpus_hard).
        framework::cpu::cpu_halt();
    }
    vprintf_unlocked(format_args!(
        "panic: {}\ncpuid = {}\n",
        info.message(),
        framework::cpu::curcpu().as_u32()
    ));
    if let Some(loc) = info.location() {
        vprintf_unlocked(format_args!("  at {}:{}\n", loc.file(), loc.line()));
    }
    vprintf_unlocked(format_args!("Halted.\n"));
    framework::cpu::cpu_halt()
}

/// Stops the machine in an orderly way (`kern_reboot(RB_HALT)`, for now
/// without syncing anything since there is nothing to sync).
pub fn kern_halt() -> ! {
    crate::printf!("The operating system has halted.\n");
    framework::cpu::cpu_halt()
}
