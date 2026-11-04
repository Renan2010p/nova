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

//! The legacy 8259A interrupt controllers (FreeBSD's `x86/isa/atpic.c`).
//!
//! Nova uses the local APICs, so the 8259s are only masked, as FreeBSD
//! does when it runs with APICs. It matters when booting through the BIOS:
//! the loader's BTX remaps them to vectors 0x20-0x2f and leaves the BIOS
//! timer running on IRQ 0, which would otherwise arrive on a vector Nova
//! never installed (under bhyve's userboot there is no BIOS, so it never
//! did; QEMU showed it).

use crate::cpufunc::outb;

/// Master controller's ports (`IO_ICU1`).
const IO_ICU1: u16 = 0x20;
/// Slave controller's ports (`IO_ICU2`).
const IO_ICU2: u16 = 0xa0;
/// Interrupt mask register, relative to the base (`ICU_IMR_OFFSET`).
const ICU_IMR_OFFSET: u16 = 1;

/// Masks every 8259 interrupt line.
pub fn atpic_mask_all() {
    // SAFETY: the interrupt mask registers of the PC's two 8259s, at fixed
    // ports; masking every line only stops interrupts nobody handles.
    unsafe {
        outb(IO_ICU1 + ICU_IMR_OFFSET, 0xff);
        outb(IO_ICU2 + ICU_IMR_OFFSET, 0xff);
    }
}
