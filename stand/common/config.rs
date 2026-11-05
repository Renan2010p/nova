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


//! The configuration file, `stand.conf`, in Limine's syntax:
//!
//! ```text
//! timeout: 5
//! interface_branding: My computer
//!
//! /Renux Nova
//!     protocol: freebsd
//!     path: boot():/boot/kernel/kernel
//!     cmdline: -v
//!     comment: Boot with every kernel message shown
//! ```
//!
//! Options are `key: value` lines (keys in any case, `#` starts a
//! comment); those before the first entry are global. An entry starts
//! with a line `/Name`; its options follow. The core reads only the
//! options every entry has (`protocol`, `comment`); the rest are the
//! protocol's.
//!
//! TODO(config): sub-entries (`//Name`, `/+Name`) are shown flat; macros
//! (`${NAME}`), other volumes than `boot()`.

/// The parsed file: borrowed text, read on demand.
#[derive(Clone, Copy, Debug)]
pub struct Config<'a> {
    text: &'a str,
}

/// An entry: a name and its option lines.
#[derive(Clone, Copy, Debug)]
pub struct Entry<'a> {
    /// Its name, as shown in the menu.
    pub name: &'a str,
    body: &'a str,
}

/// The first line of `s` and the rest.
fn split_line(s: &str) -> (&str, &str) {
    match s.split_once('\n') {
        Some((line, rest)) => (line, rest),
        None => (s, ""),
    }
}

fn is_entry(line: &str) -> bool {
    line.trim_start().starts_with('/')
}

/// The values of `key` among the option lines of `body`.
fn options<'a>(body: &'a str, key: &'a str) -> impl Iterator<Item = &'a str> + 'a {
    body.lines().filter_map(move |line| {
        let line = line.trim();
        if line.starts_with('#') {
            return None;
        }
        let (k, v) = line.split_once(':')?;
        k.trim().eq_ignore_ascii_case(key).then(|| v.trim())
    })
}

/// Splits `text` at the first line that starts an entry.
fn until_entry(text: &str) -> (&str, &str) {
    let mut rest = text;
    while !rest.is_empty() {
        let (line, next) = split_line(rest);
        if is_entry(line) {
            break;
        }
        rest = next;
    }
    let at = text.len() - rest.len();
    (text.get(..at).unwrap_or(""), rest)
}

impl<'a> Config<'a> {
    /// The configuration in `text`.
    pub fn new(text: &'a str) -> Self {
        Self { text }
    }

    /// A global option.
    pub fn get(&self, key: &'a str) -> Option<&'a str> {
        options(until_entry(self.text).0, key).next()
    }

    /// The entries, in order.
    pub fn entries(&self) -> Entries<'a> {
        Entries { rest: until_entry(self.text).1 }
    }
}

/// The entries of a [`Config`].
#[derive(Clone, Debug)]
pub struct Entries<'a> {
    rest: &'a str,
}

impl<'a> Iterator for Entries<'a> {
    type Item = Entry<'a>;

    fn next(&mut self) -> Option<Entry<'a>> {
        if self.rest.is_empty() {
            return None;
        }
        let (line, after) = split_line(self.rest);
        let (body, rest) = until_entry(after);
        self.rest = rest;
        let name = line.trim().trim_start_matches('/').trim_start_matches(['+', '-']).trim();
        Some(Entry { name, body })
    }
}

impl<'a> Entry<'a> {
    /// The entry's option `key` (the first, if it is repeated).
    pub fn get(&self, key: &'a str) -> Option<&'a str> {
        options(self.body, key).next()
    }

    /// Every value of `key` (`module_path`, for one, may be repeated).
    pub fn get_all(&self, key: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        options(self.body, key)
    }
}

/// The path on the boot volume a resource names: `boot():/path`, or a
/// bare `/path`. `None` for anything else (other volumes, not yet).
pub fn boot_path(resource: &str) -> Option<&str> {
    let path = resource.strip_prefix("boot():").unwrap_or(resource);
    path.starts_with('/').then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = "# global\ntimeout: 3\nINTERFACE_BRANDING: Test\n\n/First\n    protocol: a\n    comment: one: two\n    module_path: boot():/m1\n    module_path: /m2\n//Second\n    protocol: b\n";

    #[test]
    fn globals_and_entries() {
        let c = Config::new(TEXT);
        assert_eq!(c.get("timeout"), Some("3"));
        assert_eq!(c.get("interface_branding"), Some("Test"));
        assert_eq!(c.get("protocol"), None);
        let mut e = c.entries();
        let first = e.next().unwrap();
        assert_eq!(first.name, "First");
        assert_eq!(first.get("comment"), Some("one: two"));
        let mods: [Option<&str>; 2] = {
            let mut m = first.get_all("module_path").map(|v| boot_path(v));
            [m.next().flatten(), m.next().flatten()]
        };
        assert_eq!(mods, [Some("/m1"), Some("/m2")]);
        let second = e.next().unwrap();
        assert_eq!((second.name, second.get("protocol")), ("Second", Some("b")));
        assert!(e.next().is_none());
        assert_eq!(boot_path("hdd(1:2):/x"), None);
    }
}
