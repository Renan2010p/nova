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

//! Boot-time tunables (FreeBSD's `kern/subr_param.c`).
//!
//! Tunables come from the loader's environment (`loader.conf`), with the
//! same names and defaults as FreeBSD, so a Renux configuration means the
//! same thing under Nova.

use crate::kern_environment::StaticEnv;
use crate::kern_shutdown::PANIC_REBOOT_WAIT_TIME_DEFAULT;

/// Default clock frequency on bare metal (FreeBSD's `HZ`).
pub const HZ: u32 = 1000;
/// Default clock frequency under a hypervisor (FreeBSD's `HZ_VM`): fewer
/// timer exits for the host.
pub const HZ_VM: u32 = 100;

/// Parameters decided before anything else starts (`init_param1`).
#[derive(Clone, Copy, Debug)]
pub struct Param1 {
    /// Clock ticks per second: the `kern.hz` tunable, else `HZ`, or
    /// `HZ_VM` under a hypervisor.
    pub hz: u32,
    /// `kern.panic_reboot_wait_time`: seconds before rebooting after a
    /// panic (-1: wait for a key; 0: at once).
    pub panic_reboot_wait_time: i32,
}

/// Computes the early parameters from the static environment.
pub fn init_param1(kenv: &StaticEnv<'_>) -> Param1 {
    let default_hz = if framework::cpu::vm_guest() {
        HZ_VM
    } else {
        HZ
    };
    let hz = kenv
        .getenv_u32("kern.hz")
        .filter(|&hz| hz > 0)
        .unwrap_or(default_hz);
    let panic_reboot_wait_time = kenv
        .getenv("kern.panic_reboot_wait_time")
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(PANIC_REBOOT_WAIT_TIME_DEFAULT);
    Param1 {
        hz,
        panic_reboot_wait_time,
    }
}
