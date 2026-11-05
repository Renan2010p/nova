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

//! The firmware console (`EFI_SIMPLE_TEXT_OUTPUT_PROTOCOL`), as a
//! `fmt::Write`: what the loader prints before the kernel takes over.

#![allow(unsafe_code)]

use core::ffi::c_void;
use core::fmt;

use crate::table::{Status, system_table};

/// `EFI_SIMPLE_TEXT_OUTPUT_PROTOCOL`.
#[repr(C)]
pub(crate) struct SimpleTextOutput {
    reset: *mut c_void,
    output_string: unsafe extern "efiapi" fn(*mut SimpleTextOutput, *const u16) -> Status,
    test_string: *mut c_void,
    query_mode: *mut c_void,
    set_mode: *mut c_void,
    set_attribute: *mut c_void,
    clear_screen: unsafe extern "efiapi" fn(*mut SimpleTextOutput) -> Status,
}

impl Console {
    /// Clears the firmware console (and the screen under it).
    pub fn clear() {
        let Ok(st) = crate::table::boot_services().and_then(|_| system_table()) else { return };
        if !st.con_out.is_null() {
            // SAFETY: the firmware's console while the boot services exist.
            unsafe { ((*st.con_out).clear_screen)(st.con_out) };
        }
    }
}

/// The firmware console. Writing after the boot services ended does
/// nothing (the console is gone).
pub struct Console;

impl fmt::Write for Console {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let Ok(st) = crate::table::boot_services().and_then(|_| system_table()) else {
            return Ok(());
        };
        let out = st.con_out;
        if out.is_null() {
            return Ok(());
        }
        // UCS-2, NUL-terminated, a chunk at a time; `\n` becomes `\r\n`.
        let mut buf = [0u16; 128];
        let mut n = 0;
        let flush = |buf: &mut [u16; 128], n: &mut usize| {
            buf[*n] = 0;
            // SAFETY: `out` is the firmware's console protocol while the
            // boot services exist (checked above); `buf` is NUL-terminated.
            unsafe { (( *out).output_string)(out, buf.as_ptr()) };
            *n = 0;
        };
        for c in s.chars() {
            let mut units = [0u16; 2];
            let enc: &[u16] = if c == '\n' { &[0x0d, 0x0a] } else { c.encode_utf16(&mut units) };
            if n + enc.len() >= buf.len() - 1 {
                flush(&mut buf, &mut n);
            }
            for &u in enc {
                buf[n] = u;
                n += 1;
            }
        }
        flush(&mut buf, &mut n);
        Ok(())
    }
}
