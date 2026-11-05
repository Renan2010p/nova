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

//! The kernel's identity and settings that programs read (FreeBSD's
//! `kern/kern_mib.c`), by name. ABIs publish them under their own sysctl
//! numbers and names.


use framework::sync::MtxSpin;
use sys::errno::{Errno, KResult};

use crate::vers::{KERN_IDENT, OSRELEASE, VCS};

/// Longest host name (`MAXHOSTNAMELEN - 1`).
pub const MAXHOSTNAMELEN: usize = 255;

struct Hostname {
    buf: [u8; MAXHOSTNAMELEN],
    len: usize,
}

/// The host name (`hostname`), empty until set, as in FreeBSD.
static HOSTNAME: MtxSpin<Hostname> = MtxSpin::new(Hostname { buf: [0; MAXHOSTNAMELEN], len: 0 });

/// Copies the host name into `out`, returning its length.
pub fn hostname(out: &mut [u8; MAXHOSTNAMELEN]) -> usize {
    let h = HOSTNAME.lock();
    *out = h.buf;
    h.len
}

/// Sets the host name (`sethostname`). `ENAMETOOLONG` is not in `Errno`
/// yet: too long a name is `EINVAL`.
pub fn set_hostname(name: &[u8]) -> KResult<()> {
    if name.len() > MAXHOSTNAMELEN {
        return Err(Errno::EINVAL);
    }
    let mut h = HOSTNAME.lock();
    h.buf = [0; MAXHOSTNAMELEN];
    h.buf[..name.len()].copy_from_slice(name);
    h.len = name.len();
    Ok(())
}

/// The kernel's version line (`version[]`, `kern.version`), e.g.
/// `Nova 0.1.0-CURRENT main-n25-... GENERIC`, with FreeBSD's trailing
/// newline.
pub fn version(out: &mut impl core::fmt::Write) -> core::fmt::Result {
    writeln!(out, "Nova {OSRELEASE} {VCS} {KERN_IDENT}")
}

/// A fixed buffer that implements `fmt::Write`, for short strings.
pub struct FixedBuf<const N: usize> {
    /// The bytes written.
    pub buf: [u8; N],
    /// How many.
    pub len: usize,
}

impl<const N: usize> FixedBuf<N> {
    /// An empty buffer.
    pub const fn new() -> Self {
        Self { buf: [0; N], len: 0 }
    }

    /// What was written.
    pub fn as_bytes(&self) -> &[u8] {
        self.buf.get(..self.len).unwrap_or(&[])
    }
}

impl<const N: usize> Default for FixedBuf<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> core::fmt::Write for FixedBuf<N> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let end = self.len.checked_add(s.len()).ok_or(core::fmt::Error)?;
        let dst = self.buf.get_mut(self.len..end).ok_or(core::fmt::Error)?;
        dst.copy_from_slice(s.as_bytes());
        self.len = end;
        Ok(())
    }
}

/// Memory sizes found at boot, in bytes (`realmem`, `physmem`).
static REALMEM: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static PHYSMEM: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Records the memory sizes `cpu_startup` reports: installed (`realmem`)
/// and available to the kernel (`physmem`).
pub fn set_memory(realmem: u64, physmem: u64) {
    REALMEM.store(realmem, core::sync::atomic::Ordering::Relaxed);
    PHYSMEM.store(physmem, core::sync::atomic::Ordering::Relaxed);
}

/// Installed memory (`hw.realmem`), in bytes.
pub fn realmem() -> u64 {
    REALMEM.load(core::sync::atomic::Ordering::Relaxed)
}

/// Memory the kernel manages (`hw.physmem`), in bytes.
pub fn physmem() -> u64 {
    PHYSMEM.load(core::sync::atomic::Ordering::Relaxed)
}
