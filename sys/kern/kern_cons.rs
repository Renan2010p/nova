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

//! The MI console layer (FreeBSD's `kern/kern_cons.c`).
//!
//! For now every byte goes to and comes from the MD early console. Later
//! this layer will keep a list of console drivers (UART, framebuffer,
//! keyboard...) attached from `sys/dev`, and switch to them.
//!
//! TODO(cons): honour `RB_MUTE` and `RB_MUTEMSGS` (`boot_mute` in
//! loader.conf) like FreeBSD's `cn_mute`. Renux sets `boot_mute="YES"` by
//! default, so a drop-in kernel must keep the console quiet. Not done yet
//! because during bring-up the output is all we have.

/// Writes one byte of a kernel message to every console, turning `\n`
/// into `\r\n` on the serial line (`cnputc`).
pub fn cnputc(c: u8) {
    cnputc_attr(c, vt::Attr::Kernel);
}

/// Writes one byte to every console, drawn on the video console with
/// `attr` (kernel messages and program output look different there).
pub fn cnputc_attr(c: u8, attr: vt::Attr) {
    if c == b'\n' {
        framework::cons::early_putc(b'\r');
    }
    framework::cons::early_putc(c);
    vt::vt_putc(c, attr);
}

/// Writes a string to the console (`cnputs`).
pub fn cnputs(s: &str) {
    s.bytes().for_each(cnputc);
}

/// A byte typed on the console, if any (`cncheckc`). Never blocks.
pub fn cncheckc() -> Option<u8> {
    framework::cons::early_checkc()
}

/// Waits for a byte from the console (`cngetc`), polling; works with
/// interrupts off.
pub fn cngetc() -> u8 {
    loop {
        if let Some(c) = cncheckc() {
            return c;
        }
        framework::cpu::delay(10_000);
    }
}
