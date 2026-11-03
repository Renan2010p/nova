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

//! `boothowto` flags (FreeBSD's `sys/reboot.h`).
//!
//! These are part of the boot protocol: the loader passes them in the
//! `MODINFOMD_HOWTO` record (`boot -s`, `boot -v`, ...). The values must
//! match FreeBSD's because the loader is FreeBSD's (and the future Nova
//! stand speaks the same protocol).

/// Prompt for the root file system device.
pub const RB_ASKNAME: u32 = 0x001;
/// Boot to single-user mode.
pub const RB_SINGLE: u32 = 0x002;
/// Do not sync before reboot.
pub const RB_NOSYNC: u32 = 0x004;
/// Halt instead of rebooting.
pub const RB_HALT: u32 = 0x008;
/// Use the compiled-in root device.
pub const RB_DFLTROOT: u32 = 0x020;
/// Enter the kernel debugger early.
pub const RB_KDB: u32 = 0x040;
/// Mount root read-only.
pub const RB_RDONLY: u32 = 0x080;
/// Dump kernel memory before rebooting.
pub const RB_DUMP: u32 = 0x100;
/// Verbose boot: print all potentially useful information.
pub const RB_VERBOSE: u32 = 0x800;
/// Use the serial port as console.
pub const RB_SERIAL: u32 = 0x1000;
/// Use the CD-ROM as root.
pub const RB_CDROM: u32 = 0x2000;
/// Power off when halting.
pub const RB_POWEROFF: u32 = 0x4000;
/// Use the GDB remote debugger instead of the in-kernel one.
pub const RB_GDB: u32 = 0x8000;
/// Start with the console muted.
pub const RB_MUTE: u32 = 0x10000;
/// Pause after each line during probe.
pub const RB_PAUSE: u32 = 0x100000;
/// Unmount and remount the root file system.
pub const RB_REROOT: u32 = 0x200000;
/// Power-cycle if possible.
pub const RB_POWERCYCLE: u32 = 0x400000;
/// Start with the console muted after the banner.
pub const RB_MUTEMSGS: u32 = 0x800000;
/// Probe multiple consoles.
pub const RB_PROBE: u32 = 0x10000000;
/// Use multiple consoles.
pub const RB_MULTIPLE: u32 = 0x20000000;
