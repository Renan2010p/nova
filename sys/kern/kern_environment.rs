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

//! The kernel environment, `kenv` (FreeBSD's `kern/kern_environment.c`).
//!
//! The loader passes its variables (`loader.conf`, `set` at the prompt,
//! `smbios.*`, ...) as consecutive NUL-terminated `name=value` strings,
//! ending with an empty string. This is the read-only *static* environment;
//! the dynamic, writable one needs the kernel allocator and comes later.

/// A view of a static environment block.
#[derive(Clone, Copy, Debug)]
pub struct StaticEnv<'a> {
    buf: &'a [u8],
}

impl<'a> StaticEnv<'a> {
    /// Wraps a block, trimming it at the terminating empty string
    /// (`init_static_kenv(envp, 0)`).
    pub fn new(raw: &'a [u8]) -> Self {
        let mut end = 0;
        while end < raw.len() && raw[end] != 0 {
            match raw[end..].iter().position(|&b| b == 0) {
                Some(n) => end += n + 1,
                None => end = raw.len(),
            }
        }
        Self { buf: &raw[..end] }
    }

    /// Every `(name, value)` pair, in order. Entries that are not valid
    /// UTF-8 or have no `=` are skipped.
    pub fn iter(&self) -> impl Iterator<Item = (&'a str, &'a str)> + 'a {
        self.buf
            .split(|&b| b == 0)
            .filter_map(|e| core::str::from_utf8(e).ok())
            .filter_map(|e| e.split_once('='))
    }

    /// The value of `name` (`kern_getenv`).
    pub fn getenv(&self, name: &str) -> Option<&'a str> {
        self.iter().find(|&(n, _)| n == name).map(|(_, v)| v)
    }

    /// The value of `name` as a number (`getenv_int`): decimal, or hex
    /// with a `0x` prefix. `None` if unset or not a number.
    pub fn getenv_u32(&self, name: &str) -> Option<u32> {
        let v = self.getenv(name)?.trim();
        match v.strip_prefix("0x").or_else(|| v.strip_prefix("0X")) {
            Some(hex) => u32::from_str_radix(hex, 16).ok(),
            None => v.parse().ok(),
        }
    }

    /// Number of variables.
    pub fn len(&self) -> usize {
        self.iter().count()
    }

    /// True if there are no variables.
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }
}
