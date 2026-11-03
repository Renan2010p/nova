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

//! The static kernel environment (FreeBSD's `kern/kern_environment.c`,
//! the part that works before `malloc`).
//!
//! The loader passes its variables (`loader.conf`, `set` at the prompt,
//! `smbios.*`, `acpi.rsdp`, ...) as consecutive NUL-terminated
//! `name=value` strings, ending with an empty string. It is defined here,
//! in `sys`, because MD code needs it too (`acpi.rsdp`).

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

    /// The value of `name` as a number (`getenv_quad`): decimal, or hex
    /// with a `0x` prefix. `None` if unset or not a number.
    pub fn getenv_u64(&self, name: &str) -> Option<u64> {
        let v = self.getenv(name)?.trim();
        match v.strip_prefix("0x").or_else(|| v.strip_prefix("0X")) {
            Some(hex) => u64::from_str_radix(hex, 16).ok(),
            None => v.parse().ok(),
        }
    }

    /// The value of `name` as a 32-bit number (`getenv_int`).
    pub fn getenv_u32(&self, name: &str) -> Option<u32> {
        self.getenv_u64(name).and_then(|v| u32::try_from(v).ok())
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

#[cfg(test)]
mod tests {
    use super::StaticEnv;

    #[test]
    fn parses_and_trims() {
        let raw = b"kernelname=/boot/kernel/kernel\0acpi.rsdp=0x000f2400\0kern.hz=100\0bad\0\0garbage=1\0";
        let env = StaticEnv::new(raw);
        assert_eq!(env.len(), 3);
        assert_eq!(env.getenv("kernelname"), Some("/boot/kernel/kernel"));
        assert_eq!(env.getenv_u64("acpi.rsdp"), Some(0xf2400));
        assert_eq!(env.getenv_u32("kern.hz"), Some(100));
        assert_eq!(env.getenv("garbage"), None);
        assert!(StaticEnv::new(b"\0").is_empty());
    }
}
