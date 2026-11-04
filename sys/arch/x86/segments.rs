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

//! Segment and gate descriptor encodings (FreeBSD's
//! `x86/include/segments.h`).
//!
//! Only the encodings live here. Which descriptor goes in which GDT slot
//! is part of the amd64 ABI and lives in `amd64::segments`.

use libkern::conv::{trunc_u16, u64_from_u16, u64_from_u8, u64_from_usize};

/// Kernel privilege level.
pub const SEL_KPL: u16 = 0;
/// User privilege level.
pub const SEL_UPL: u16 = 3;

/// Selector of GDT slot `slot` with requested privilege level `rpl`
/// (FreeBSD's `GSEL`).
pub const fn gsel(slot: usize, rpl: u16) -> u16 {
    (trunc_u16(u64_from_usize(slot)) << 3) | rpl // slot < 8192
}

/// 64-bit code segment, ring 0.
pub const SD_KCODE64: u64 = 0x00af_9b00_0000_ffff;
/// Data segment, ring 0.
pub const SD_KDATA: u64 = 0x00cf_9300_0000_ffff;
/// 32-bit code segment, ring 3 (for 32-bit binaries).
pub const SD_UCODE32: u64 = 0x00cf_fb00_0000_ffff;
/// Data segment, ring 3.
pub const SD_UDATA: u64 = 0x00cf_f300_0000_ffff;
/// 64-bit code segment, ring 3.
pub const SD_UCODE64: u64 = 0x00af_fb00_0000_ffff;

/// System descriptor type: available 64-bit TSS.
pub const SDT_SYSTSS: u64 = 9;
/// System descriptor type: 64-bit interrupt gate (clears IF on entry).
pub const SDT_SYSIGT: u64 = 14;

/// The two GDT slots of a 64-bit TSS descriptor for a TSS at `base` of
/// `size` bytes (FreeBSD's `ssdtosyssd`).
pub const fn tss_descriptor(base: u64, size: u64) -> [u64; 2] {
    let limit = size - 1;
    let low = (limit & 0xffff)
        | ((base & 0x00ff_ffff) << 16)
        | ((SDT_SYSTSS | 0x80) << 40) // present, DPL 0
        | (((limit >> 16) & 0xf) << 48)
        | (((base >> 24) & 0xff) << 56);
    [low, base >> 32]
}

/// A 64-bit IDT gate (FreeBSD's `struct gate_descriptor`), as two words.
pub const fn gate_descriptor(offset: u64, sel: u16, ist: u8, typ: u64, dpl: u16) -> [u64; 2] {
    let low = (offset & 0xffff)
        | (u64_from_u16(sel) << 16)
        | (u64_from_u8(ist & 7) << 32)
        | ((typ | (u64_from_u16(dpl) << 5) | 0x80) << 40) // present
        | (((offset >> 16) & 0xffff) << 48);
    [low, offset >> 32]
}

#[cfg(test)]
#[allow(clippy::as_conversions, reason = "host test fixtures, not kernel code")]
mod tests {
    use super::*;

    #[test]
    // User selectors are visible to programs (signal frames, %cs), so they
    // are fixed: those of FreeBSD's GDT layout.
    fn selectors_are_fixed() {
        assert_eq!(gsel(4, SEL_KPL), 0x20);
        assert_eq!(gsel(8, SEL_UPL), 0x43);
    }

    #[test]
    fn gate_layout() {
        let [lo, hi] = gate_descriptor(0xffff_ffff_8020_1234, 0x20, 1, SDT_SYSIGT, 0);
        assert_eq!(lo & 0xffff, 0x1234);
        assert_eq!((lo >> 16) & 0xffff, 0x20);
        assert_eq!((lo >> 32) & 7, 1);
        assert_eq!((lo >> 40) & 0xff, 0x8e);
        assert_eq!(lo >> 48, 0x8020);
        assert_eq!(hi, 0xffff_ffff);
    }

    #[test]
    fn tss_layout() {
        let [lo, hi] = tss_descriptor(0xffff_ffff_8123_4560, 104);
        assert_eq!(lo & 0xffff, 103);
        assert_eq!((lo >> 16) & 0xff_ffff, 0x23_4560);
        assert_eq!((lo >> 40) & 0xff, 0x89);
        assert_eq!(lo >> 56, 0x81);
        assert_eq!(hi, 0xffff_ffff);
    }
}
