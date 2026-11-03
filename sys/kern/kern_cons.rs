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
//! For now every byte goes to the MD early console. Later this layer will
//! keep a list of console drivers (UART, framebuffer, ...) attached from
//! `sys/dev`, and switch from the early console to them.
//!
//! TODO(cons): honour `RB_MUTE` and `RB_MUTEMSGS` (`boot_mute` in
//! loader.conf) like FreeBSD's `cn_mute`. Renux sets `boot_mute="YES"` by
//! default, so a drop-in kernel must keep the console quiet. Not done yet
//! because during bring-up the output is all we have.

/// Writes one byte to the console, turning `\n` into `\r\n` (`cnputc`).
pub fn cnputc(c: u8) {
    if c == b'\n' {
        framework::cons::early_putc(b'\r');
    }
    framework::cons::early_putc(c);
}

/// Writes a string to the console (`cnputs`).
pub fn cnputs(s: &str) {
    s.bytes().for_each(cnputc);
}
