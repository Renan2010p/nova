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

//! The kernel environment the loader builds (`kenv`, FreeBSD's
//! `stand/libsa/environment.c` and `md_copyenv`): `name=value` strings,
//! each NUL-terminated, ended by an empty one, exactly as the kernel reads
//! it (`sys::kenv`).

/// An environment in a caller's buffer.
pub struct Env<'a> {
    buf: &'a mut [u8],
    len: usize,
}

/// The buffer is full.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Full;

impl<'a> Env<'a> {
    /// An empty environment in `buf`.
    pub fn new(buf: &'a mut [u8]) -> Self {
        Self { buf, len: 0 }
    }

    /// The `(start, end)` of each `name=value` string.
    fn entries(&self) -> impl Iterator<Item = (usize, usize)> + '_ {
        let mut off = 0;
        core::iter::from_fn(move || {
            if off >= self.len {
                return None;
            }
            let rest = self.buf.get(off..self.len)?;
            let n = rest.iter().position(|&b| b == 0)?;
            let e = (off, off + n);
            off += n + 1;
            Some(e)
        })
    }

    /// The value of `name` (`getenv`).
    pub fn get(&self, name: &str) -> Option<&str> {
        self.entries().find_map(|(s, e)| {
            let entry = self.buf.get(s..e)?;
            let rest = entry.strip_prefix(name.as_bytes())?.strip_prefix(b"=")?;
            core::str::from_utf8(rest).ok()
        })
    }

    /// Removes `name` (`unsetenv`).
    pub fn unset(&mut self, name: &str) {
        let found = self.entries().find(|&(s, e)| {
            self.buf.get(s..e).is_some_and(|en| en.strip_prefix(name.as_bytes()).is_some_and(|r| r.first() == Some(&b'=')))
        });
        if let Some((s, e)) = found {
            self.buf.copy_within(e + 1..self.len, s);
            self.len -= e + 1 - s;
        }
    }

    /// Sets `name` to `value` (`setenv` with overwrite).
    pub fn set(&mut self, name: &str, value: &str) -> Result<(), Full> {
        self.unset(name);
        let need = name.len() + 1 + value.len() + 1;
        let end = self.len.checked_add(need).filter(|&e| e < self.buf.len()).ok_or(Full)?;
        let dst = self.buf.get_mut(self.len..end).ok_or(Full)?;
        dst[..name.len()].copy_from_slice(name.as_bytes());
        dst[name.len()] = b'=';
        dst[name.len() + 1..need - 1].copy_from_slice(value.as_bytes());
        dst[need - 1] = 0;
        self.len = end;
        Ok(())
    }

    /// The environment as the kernel reads it: the strings and the final
    /// empty one (`md_copyenv`).
    pub fn as_kernel_bytes(&mut self) -> Result<&[u8], Full> {
        *self.buf.get_mut(self.len).ok_or(Full)? = 0;
        self.buf.get(..=self.len).ok_or(Full)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_get_unset() {
        let mut buf = [0u8; 128];
        let mut e = Env::new(&mut buf);
        e.set("a", "1").unwrap();
        e.set("boot_verbose", "YES").unwrap();
        e.set("a", "2").unwrap();
        assert_eq!(e.get("a"), Some("2"));
        assert_eq!(e.get("boot_verbose"), Some("YES"));
        assert_eq!(e.get("boot"), None);
        e.unset("a");
        assert_eq!(e.as_kernel_bytes().unwrap(), b"boot_verbose=YES\0\0");
    }
}
