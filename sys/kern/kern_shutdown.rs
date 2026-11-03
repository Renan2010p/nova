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

//! Halt, reboot and panic (FreeBSD's `kern/kern_shutdown.c`), with
//! FreeBSD's messages, so a Renux "Nova" console reads like a FreeBSD one:
//!
//! ```text
//! panic: page fault
//! cpuid = 0
//! time = 2
//! KDB: stack backtrace:
//! #0 0xffffffff8020a5d1 at kern::kern_shutdown::kdb_backtrace+0x41
//! ...
//! Uptime: 2s
//! Automatic reboot in 15 seconds - press a key on the console to abort
//! ```
//!
//! Every Rust `panic!` in the kernel, MD or MI, ends up in [`panic`]. MD
//! code can call `panic!` without depending on this crate: the handler is
//! bound at link time by the language.

use core::fmt::Write;
use core::panic::PanicInfo;
use core::sync::atomic::{AtomicBool, AtomicI32, Ordering};

use framework::StackFrame;
use sys::reboot::RB_HALT;

use crate::kern_clock;
use crate::kern_cons::{cncheckc, cngetc};
use crate::kern_linker::print_symbol;
use crate::subr_prf::{ConsUnlocked, vprintf_unlocked};

static PANICKING: AtomicBool = AtomicBool::new(false);

/// Seconds to wait before rebooting after a panic; -1 waits for a key,
/// 0 reboots at once (`kern.panic_reboot_wait_time`).
static PANIC_REBOOT_WAIT_TIME: AtomicI32 = AtomicI32::new(PANIC_REBOOT_WAIT_TIME_DEFAULT);
/// FreeBSD's `PANIC_REBOOT_WAIT_TIME`.
pub const PANIC_REBOOT_WAIT_TIME_DEFAULT: i32 = 15;

/// Sets `kern.panic_reboot_wait_time`.
pub fn set_panic_reboot_wait_time(seconds: i32) {
    PANIC_REBOOT_WAIT_TIME.store(seconds, Ordering::Relaxed);
}

/// Deepest backtrace printed.
const BACKTRACE_MAX: usize = 32;

/// The kernel panic handler (`vpanic`).
#[cfg(not(test))]
#[panic_handler]
fn panic(info: &PanicInfo<'_>) -> ! {
    // Never dropped: we stay on this CPU with interrupts off until reset.
    let _intr = framework::cpu::intr_guard();
    if PANICKING.swap(true, Ordering::AcqRel) {
        // Another CPU is panicking, or printing is what failed: just stop.
        framework::cpu::cpu_halt();
    }
    // The other CPUs stop where they are (`stop_cpus_hard`).
    framework::smp::stop_cpus();
    vprintf_unlocked(format_args!("panic: {}\n", info.message()));
    if let Some(loc) = info.location() {
        vprintf_unlocked(format_args!("  at {}:{}\n", loc.file(), loc.line()));
    }
    vprintf_unlocked(format_args!(
        "cpuid = {}\n",
        framework::cpu::curcpu().as_u32()
    ));
    vprintf_unlocked(format_args!("time = {}\n", kern_clock::uptime_secs()));
    kdb_backtrace();
    print_uptime();
    shutdown_panic();
    shutdown_reset()
}

/// Prints the stack of the caller (`kdb_backtrace`), in the format of
/// FreeBSD's `db_backtrace`: one `#n 0x... at function+0x...` line per
/// frame, and `--- trap 0xc, rip = ..., rsp = ..., rbp = ... ---` where a
/// trap or interrupt was taken.
#[inline(never)]
fn kdb_backtrace() {
    vprintf_unlocked(format_args!("KDB: stack backtrace:\n"));
    let mut frame = 0;
    framework::cpu::stack_trace(&mut |f| {
        let mut w = ConsUnlocked;
        let (pc, is_return) = match f {
            StackFrame::Return(pc) => (pc, true),
            StackFrame::Pc(pc) => (pc, false),
            StackFrame::Trap {
                interrupt,
                trapno,
                pc,
                sp,
                fp,
            } => {
                let _ = if interrupt {
                    w.write_str("--- interrupt")
                } else {
                    write!(w, "--- trap {trapno:#x}")
                };
                let _ = writeln!(w, ", rip = {pc:#x}, rsp = {sp:#x}, rbp = {fp:#x} ---");
                return true;
            }
        };
        let _ = write!(w, "#{frame} {pc:#x} at ");
        let _ = print_symbol(&mut w, pc, is_return);
        let _ = w.write_str("\n");
        frame += 1;
        frame < BACKTRACE_MAX
    });
}

/// `Uptime: 1h2m3s` (`print_uptime`).
fn print_uptime() {
    let mut s = kern_clock::uptime_secs();
    let mut w = ConsUnlocked;
    let _ = w.write_str("Uptime: ");
    let mut shown = false;
    for (unit, len) in [("d", 86_400), ("h", 3_600), ("m", 60)] {
        if shown || s >= len {
            let _ = write!(w, "{}{unit}", s / len);
            s %= len;
            shown = true;
        }
    }
    let _ = writeln!(w, "{s}s");
}

/// After a panic: wait so the message can be read (`shutdown_panic`).
fn shutdown_panic() {
    let wait = PANIC_REBOOT_WAIT_TIME.load(Ordering::Relaxed);
    if wait == 0 {
        return;
    }
    if wait > 0 {
        vprintf_unlocked(format_args!(
            "Automatic reboot in {wait} seconds - press a key on the console to abort\n"
        ));
        let mut aborted = false;
        for _ in 0..wait * 10 {
            framework::cpu::delay(100_000); // 1/10th second
            if cncheckc().is_some() {
                aborted = true;
                break;
            }
        }
        if !aborted {
            return;
        }
    }
    vprintf_unlocked(format_args!("--> Press a key on the console to reboot,\n"));
    vprintf_unlocked(format_args!("--> or switch off the system now.\n"));
    cngetc();
}

/// `shutdown_reset`.
fn shutdown_reset() -> ! {
    vprintf_unlocked(format_args!("Rebooting...\n"));
    framework::cpu::cpu_reset()
}

/// Stops the system (`kern_reboot`). With `RB_HALT`, waits for a key
/// before resetting, as FreeBSD's `shutdown_halt` does.
///
/// TODO(vfs): sync and unmount file systems first.
pub fn kern_reboot(howto: u32) -> ! {
    print_uptime();
    if howto & RB_HALT != 0 {
        crate::printf!("The operating system has halted.\n");
        crate::printf!("Please press any key to reboot.\n\n");
        cngetc();
    }
    shutdown_reset()
}
