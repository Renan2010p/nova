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

//! The firmware keyboard (`EFI_SIMPLE_TEXT_INPUT_PROTOCOL`): keys from
//! the keyboard and, on most firmware, the serial line too.

#![allow(unsafe_code)]

use core::ffi::c_void;

use crate::table::{Status, boot_services, system_table};

/// `EFI_SIMPLE_TEXT_INPUT_PROTOCOL`.
#[repr(C)]
pub(crate) struct SimpleTextInput {
    reset: *mut c_void,
    read_key_stroke: unsafe extern "efiapi" fn(*mut SimpleTextInput, *mut InputKey) -> Status,
}

#[repr(C)]
struct InputKey {
    scan_code: u16,
    unicode_char: u16,
}

/// A key press.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    /// Up arrow.
    Up,
    /// Down arrow.
    Down,
    /// Right arrow.
    Right,
    /// Home.
    Home,
    /// End.
    End,
    /// Escape.
    Escape,
    /// Enter (carriage return or line feed).
    Enter,
    /// Any other character.
    Char(char),
    /// A key with no character the loader knows.
    Other,
}

/// The next key pressed, if any (never waits).
pub fn read_key() -> Option<Key> {
    boot_services().ok()?;
    let st = system_table().ok()?;
    let input = st.con_in;
    if input.is_null() {
        return None;
    }
    let mut k = InputKey { scan_code: 0, unicode_char: 0 };
    // SAFETY: the firmware's input protocol while the boot services exist
    // (checked above); `k` is a local it fills in.
    let status = unsafe { ((*input).read_key_stroke)(input, &mut k) };
    if status != Status::SUCCESS {
        return None;
    }
    Some(match (k.scan_code, k.unicode_char) {
        (1, _) => Key::Up,
        (2, _) => Key::Down,
        (3, _) => Key::Right,
        (5, _) => Key::Home,
        (6, _) => Key::End,
        (0x17, _) => Key::Escape,
        (0, 0x0d | 0x0a) => Key::Enter,
        (0, c) => char::from_u32(u32::from(c)).map_or(Key::Other, Key::Char),
        _ => Key::Other,
    })
}
