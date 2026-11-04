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

//! Memory mapping system calls (FreeBSD's `vm/vm_mmap.c`), as the kernel
//! implements them; ABIs decode `mmap(2)` and friends into these.
//!
//! TODO(vfs): file mappings; TODO(vm): `mprotect`, copy-on-write sharing.

use framework::user::{Inherit, MapPlace, Prot, VmError, VmSpace};
use sys::errno::{Errno, KResult};

/// Where mappings without an address go, by default: above the program's
/// data and its room to grow (FreeBSD starts after `vm_daddr + maxdsiz`;
/// Nova uses a fixed base until it has resource limits).
pub const MMAP_BASE: u64 = 0x0000_0008_0000_0000;

/// What to map (`mmap` flags, by name).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MmapAt {
    /// Anywhere at or above the hint, aligned to `2^align_shift` bytes.
    Hint {
        /// The hint (0: the default base).
        addr: u64,
        /// log2 of the alignment (`MAP_ALIGNED`); 0 for a page.
        align_shift: u32,
    },
    /// Exactly there, replacing earlier mappings (`MAP_FIXED`).
    Fixed(u64),
    /// Exactly there, if nothing is (`MAP_FIXED | MAP_EXCL`).
    FixedNoReplace(u64),
}

/// The error a system call reports for a [`VmError`].
pub fn vm_errno(e: VmError) -> Errno {
    match e {
        VmError::NoMemory => Errno::ENOMEM,
        VmError::Invalid | VmError::Exists => Errno::EINVAL,
        VmError::Fault => Errno::EFAULT,
    }
}

/// Maps `len` bytes of zero-filled, private, anonymous memory
/// (`kern_mmap` with `MAP_ANON | MAP_PRIVATE`). Returns the address.
pub fn kern_mmap_anon(vm: &VmSpace, at: MmapAt, len: u64, prot: Prot) -> KResult<u64> {
    if len == 0 {
        return Err(Errno::EINVAL);
    }
    let r = match at {
        MmapAt::Hint { addr, align_shift } => {
            let hint = if addr == 0 { MMAP_BASE } else { addr & !0xfff };
            vm.map_anon(hint, len, prot, MapPlace::Hint { align_shift })
        }
        MmapAt::Fixed(addr) => vm.map_anon(addr, len, prot, MapPlace::Fixed),
        MmapAt::FixedNoReplace(addr) => vm.map_anon(addr, len, prot, MapPlace::FixedNoReplace),
    };
    r.map_err(|e| match e {
        // FreeBSD: no room is ENOMEM; MAP_EXCL over a mapping is EINVAL.
        VmError::NoMemory => Errno::ENOMEM,
        other => vm_errno(other),
    })
}

/// Removes mappings (`kern_munmap`).
pub fn kern_munmap(vm: &VmSpace, addr: u64, len: u64) -> KResult<()> {
    if len == 0 {
        return Err(Errno::EINVAL);
    }
    vm.unmap(addr, len).map_err(vm_errno)
}

/// Sets what a child gets of a range at `fork` (`kern_minherit`).
pub fn kern_minherit(vm: &VmSpace, addr: u64, len: u64, inherit: Inherit) -> KResult<()> {
    vm.set_inherit(addr, len, inherit).map_err(vm_errno)
}

/// Changes the protection of a range (`kern_mprotect`): `ENOMEM` if part
/// of it is not mapped.
pub fn kern_mprotect(vm: &VmSpace, addr: u64, len: u64, prot: Prot) -> KResult<()> {
    vm.protect(addr, len, prot).map_err(|e| match e {
        VmError::NoMemory => Errno::ENOMEM,
        other => vm_errno(other),
    })
}
