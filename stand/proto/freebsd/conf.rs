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

//! `loader.conf(5)` and `device.hints(5)`: `name="value"` lines that set
//! environment variables (the subset of FreeBSD's loader.conf that is
//! variables; `*_load` lines for modules are kept as variables too).
//!
//! TODO(conf): `loader.conf.d`, `exec=`, includes, `loader_conf_files`.

use crate::env::{Env, Full};

/// Sets every variable `text` assigns. Comments (`#`) and blank lines are
/// skipped; values may be quoted.
pub fn apply(text: &str, env: &mut Env<'_>) -> Result<(), Full> {
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        let Some((name, value)) = line.split_once('=') else { continue };
        let name = name.trim();
        let value = value.trim();
        let value = value.strip_prefix('"').and_then(|v| v.strip_suffix('"')).unwrap_or(value);
        if !name.is_empty() && !name.contains(char::is_whitespace) {
            env.set(name, value)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_loader_conf() {
        let mut buf = [0u8; 256];
        let mut e = Env::new(&mut buf);
        apply("# comment\nboot_verbose=\"YES\"\n  console = comconsole # serial\nhint.uart.0.at=\"isa\"\nbogus\n", &mut e).unwrap();
        assert_eq!(e.get("boot_verbose"), Some("YES"));
        assert_eq!(e.get("console"), Some("comconsole"));
        assert_eq!(e.get("hint.uart.0.at"), Some("isa"));
    }
}
