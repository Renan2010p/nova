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

//! `vt`: the video console (FreeBSD's `sys/dev/vt`), on the framebuffer
//! the firmware left on (`vt_efifb`).
//!
//! It draws what the kernel prints as FreeBSD's `vt(4)` does: the same
//! font (the loader's, or the built-in "Terminus BSD Console" 8x16), the
//! same colours (kernel messages in bold white, program output in light
//! gray, on black), the text area centred on the screen.
//!
//! A driver in the framekernel sense: 100% safe Rust, reaching the
//! hardware only through `framework::fb`, so a bug here cannot corrupt
//! kernel memory. It depends on `framework`, `sys` (fonts, preload
//! metadata) and `libkern`; `kern` calls it from the console code.
//!
//! TODO(vt): virtual terminals, keyboard input, a cursor, colours from
//! programs (escape sequences, FreeBSD's `teken`).

#![no_std]
#![forbid(unsafe_code)]

mod font;
mod vt_core;

pub use vt_core::{Attr, vt_init, vt_putc};
