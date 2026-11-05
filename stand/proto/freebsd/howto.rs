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

//! The boot flags (`boothowto`): those `boot`'s arguments give
//! (`boot_parse_cmdline`), those the environment implies
//! (`boot_env_to_howto`), and back (`boot_howto_to_env`), as FreeBSD's
//! `kern/subr_boot.c`, shared by loader and kernel; and the console
//! variable's effect on them.

use sys::reboot::{
    RB_ASKNAME, RB_CDROM, RB_DFLTROOT, RB_GDB, RB_KDB, RB_MULTIPLE, RB_MUTE, RB_MUTEMSGS, RB_PAUSE, RB_PROBE,
    RB_SERIAL, RB_SINGLE, RB_VERBOSE,
};

use crate::env::{Env, Full};

/// `howto_names[]`: variables that set a flag unless they say "no".
const HOWTO_NAMES: &[(&str, u32)] = &[
    ("boot_askname", RB_ASKNAME),
    ("boot_cdrom", RB_CDROM),
    ("boot_ddb", RB_KDB),
    ("boot_dfltroot", RB_DFLTROOT),
    ("boot_gdb", RB_GDB),
    ("boot_multicons", RB_MULTIPLE),
    ("boot_mute", RB_MUTE),
    ("boot_mutemsgs", RB_MUTEMSGS),
    ("boot_pause", RB_PAUSE),
    ("boot_serial", RB_SERIAL),
    ("boot_single", RB_SINGLE),
    ("boot_verbose", RB_VERBOSE),
];

/// `boot_parse_arg`: `-flags` give boot flags (`-S<speed>` sets
/// `comconsole_speed`); `name=value` sets a variable, a bare `name` sets
/// it to "1".
pub fn boot_parse_arg(arg: &str, env: &mut Env<'_>) -> Result<u32, Full> {
    let Some(flags) = arg.strip_prefix('-') else {
        let (name, value) = arg.split_once('=').unwrap_or((arg, "1"));
        env.set(name, value)?;
        return Ok(0);
    };
    let mut howto = 0;
    for (i, c) in flags.char_indices() {
        howto |= match c {
            'a' => RB_ASKNAME,
            'C' => RB_CDROM,
            'd' => RB_KDB,
            'D' => RB_MULTIPLE,
            'm' => RB_MUTE,
            'M' => RB_MUTEMSGS,
            'g' => RB_GDB,
            'h' => RB_SERIAL,
            'p' => RB_PAUSE,
            'P' => RB_PROBE,
            'r' => RB_DFLTROOT,
            's' => RB_SINGLE,
            'v' => RB_VERBOSE,
            'S' => {
                env.set("comconsole_speed", flags.get(i + 1..).unwrap_or(""))?;
                break;
            }
            _ => 0,
        };
    }
    Ok(howto)
}

/// `boot_parse_cmdline`: the flags of every space-separated argument.
pub fn boot_parse_cmdline(cmdline: &str, env: &mut Env<'_>) -> Result<u32, Full> {
    cmdline.split_ascii_whitespace().try_fold(0, |howto, arg| Ok(howto | boot_parse_arg(arg, env)?))
}

/// `boot_howto_to_env`: a `boot_*` variable set to "YES" for each flag.
pub fn boot_howto_to_env(howto: u32, env: &mut Env<'_>) -> Result<(), Full> {
    for (name, flag) in HOWTO_NAMES {
        if howto & flag != 0 {
            env.set(name, "YES")?;
        }
    }
    Ok(())
}

/// The flags set by `boot_*` variables (`boot_env_to_howto`).
pub fn boot_env_to_howto(env: &Env<'_>) -> u32 {
    HOWTO_NAMES
        .iter()
        .filter(|(name, _)| env.get(name).is_some_and(|v| !v.eq_ignore_ascii_case("no")))
        .fold(0, |howto, (_, flag)| howto | flag)
}

/// What `console=` means for the flags, as the loader's console code sets
/// them: the serial console gives `boot_serial`, several consoles
/// `boot_multicons`.
pub fn console_to_env(env: &mut Env<'_>) -> Result<(), Full> {
    let Some(console) = env.get("console") else { return Ok(()) };
    let serial = console.split([',', ' ']).any(|c| c == "comconsole");
    let several = console.split([',', ' ']).filter(|c| !c.is_empty()).count() > 1;
    if serial {
        env.set("boot_serial", "YES")?;
    }
    if several {
        env.set("boot_multicons", "YES")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cmdline() {
        let mut buf = [0u8; 256];
        let mut e = Env::new(&mut buf);
        let howto = boot_parse_cmdline("-sv  -S115200 vfs.root.mountfrom=ufs:/dev/ada0p2 kern.foo", &mut e).unwrap();
        assert_eq!(howto, RB_SINGLE | RB_VERBOSE);
        assert_eq!(e.get("comconsole_speed"), Some("115200"));
        assert_eq!(e.get("vfs.root.mountfrom"), Some("ufs:/dev/ada0p2"));
        assert_eq!(e.get("kern.foo"), Some("1"));
        boot_howto_to_env(howto, &mut e).unwrap();
        assert_eq!(boot_env_to_howto(&e), RB_SINGLE | RB_VERBOSE);
    }
}
