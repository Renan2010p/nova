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

//! Polled NS8250/16550 UART on a legacy PC I/O port, used as the early
//! console.
//!
//! This is the minimum needed to print before any driver framework exists.
//! The real UART driver will be MI code in `sys/dev/ic/ns8250.rs`, reaching
//! the hardware through a bus-space abstraction (the NetBSD `dev/ic` model),
//! and will take the console over from this one.

use crate::cpufunc::{ia32_pause, inb, outb};

/// I/O base of COM1, where the FreeBSD loader's `comconsole` lives by
/// default.
pub const COM1: u16 = 0x3f8;

const REG_DATA: u16 = 0; // RBR/THR, or DLL when DLAB=1
const REG_IER: u16 = 1; // interrupt enable, or DLM when DLAB=1
const REG_FCR: u16 = 2; // FIFO control
const REG_LCR: u16 = 3; // line control
const REG_MCR: u16 = 4; // modem control
const REG_LSR: u16 = 5; // line status

const LCR_DLAB: u8 = 0x80;
const LCR_8N1: u8 = 0x03;
const FCR_ENABLE_CLEAR: u8 = 0x07;
const MCR_DTR_RTS: u8 = 0x03;
const LSR_RXRDY: u8 = 0x01;
const LSR_THRE: u8 = 0x20;

/// Divisor for 115200 baud with the standard 1.8432 MHz clock.
const DIVISOR_115200: u16 = 1;

/// Give up waiting for the transmitter after this many polls, so a missing
/// or wedged UART cannot hang the kernel.
const TX_SPIN_LIMIT: u32 = 100_000;

/// A polled UART at a fixed I/O base.
pub struct Ns8250 {
    base: u16,
}

impl Ns8250 {
    /// # Safety
    ///
    /// `base` must be the I/O base of an 8250-compatible UART, or of no
    /// device at all, and nothing else may drive that UART.
    pub const unsafe fn new(base: u16) -> Self {
        Self { base }
    }

    /// Programs 115200 8N1 with FIFOs on and interrupts off.
    pub fn init(&self) {
        let b = self.base;
        // SAFETY: `new`'s contract makes these the registers of a UART we
        // own; this is the standard 16550 initialization sequence.
        unsafe {
            outb(b + REG_IER, 0);
            outb(b + REG_LCR, LCR_DLAB);
            outb(b + REG_DATA, DIVISOR_115200 as u8);
            outb(b + REG_IER, (DIVISOR_115200 >> 8) as u8);
            outb(b + REG_LCR, LCR_8N1);
            outb(b + REG_FCR, FCR_ENABLE_CLEAR);
            outb(b + REG_MCR, MCR_DTR_RTS);
        }
    }

    /// Sends one byte, waiting (bounded) for room in the transmitter.
    pub fn putc(&self, c: u8) {
        let b = self.base;
        // SAFETY: as in `init`. Reading LSR has no side effects.
        unsafe {
            for _ in 0..TX_SPIN_LIMIT {
                if inb(b + REG_LSR) & LSR_THRE != 0 {
                    break;
                }
                ia32_pause();
            }
            outb(b + REG_DATA, c);
        }
    }

    /// A received byte, if one is waiting (`cncheckc`).
    pub fn getc(&self) -> Option<u8> {
        let b = self.base;
        // SAFETY: as in `init`. Reading LSR has no side effects; reading
        // RBR consumes the byte, which is what the caller asks for.
        unsafe { (inb(b + REG_LSR) & LSR_RXRDY != 0).then(|| inb(b + REG_DATA)) }
    }
}
