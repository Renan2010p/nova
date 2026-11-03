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

//! Machine reset (FreeBSD's `cpu_reset_real` in `x86/x86/cpu_machdep.c`),
//! trying the same methods in the same order.

use x86::cpufunc::{RegionDescriptor, disable_intr, halt, lidt, outb};
use x86::tsc::delay;

/// `mdi::Cpu::cpu_reset`.
pub(crate) fn cpu_reset() -> ! {
    disable_intr();
    // SAFETY: we are resetting the machine; whatever the devices at these
    // ports do next no longer matters to the kernel.
    unsafe {
        // Pulse the reset line through the keyboard controller.
        outb(0x64, 0xfe);
        delay(500_000);
        // Reset Control register: bit 1 selects a hard reset, bit 2 does it.
        outb(0xcf9, 0x2);
        outb(0xcf9, 0x6);
        delay(500_000);
        // Last resort: a triple fault, with an empty IDT.
        lidt(&RegionDescriptor { limit: 0, base: 0 });
        core::arch::asm!("int3", options(nomem, nostack));
    }
    loop {
        halt();
    }
}
