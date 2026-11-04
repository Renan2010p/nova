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

//! ACPI tables before any driver exists: the machine-independent part of
//! what FreeBSD's ACPICA offers early (`AcpiFindRootPointer`,
//! `AcpiGetTable`).
//!
//! * the RSDP, parsed and validated, and found by scanning a memory area;
//! * the XSDT (or RSDT), which lists the other tables;
//! * any table by signature, length- and checksum-checked.
//!
//! What a table *means* is not here: the MADT's CPU entries, for example,
//! differ per architecture and are parsed in `sys/arch` (FreeBSD's
//! `x86/acpica/madt.c`). Physical memory is read through a closure the MD
//! layer provides, so this module is plain, safe parsing, unit-tested on
//! the host.

use crate::bytes::{read_u32, read_u64};

/// RSDP signature.
pub const RSDP_SIG: &[u8; 8] = b"RSD PTR ";
/// Size of the ACPI 1.0 RSDP, covered by the first checksum.
const RSDP_V1_LEN: usize = 20;
/// Size of a system description table header.
pub const SDT_HEADER_LEN: usize = 36;

/// Reads `len` bytes of physical memory at `pa`, or `None` if they cannot
/// be reached.
pub type PhysRead<'a> = &'a dyn Fn(u64, usize) -> Option<&'a [u8]>;

/// The parts of the RSDP the kernel uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rsdp {
    /// ACPI revision (0 for 1.0, 2 and later have an XSDT).
    pub revision: u8,
    /// Physical address of the RSDT.
    pub rsdt: u32,
    /// Physical address of the XSDT (0 before ACPI 2.0).
    pub xsdt: u64,
}

fn checksum_ok(b: &[u8]) -> bool {
    b.iter().fold(0u8, |s, &x| s.wrapping_add(x)) == 0
}

/// Parses and validates an RSDP.
pub fn parse_rsdp(b: &[u8]) -> Option<Rsdp> {
    if b.get(..8)? != RSDP_SIG || !checksum_ok(b.get(..RSDP_V1_LEN)?) {
        return None;
    }
    let revision = *b.get(15)?;
    let rsdt = read_u32(b, 16)?;
    let xsdt = if revision >= 2 {
        let len = read_u32(b, 20)? as usize;
        if !checksum_ok(b.get(..len)?) {
            return None;
        }
        read_u64(b, 24)?
    } else {
        0
    };
    Some(Rsdp {
        revision,
        rsdt,
        xsdt,
    })
}

/// Scans `area`, which starts at physical address `base`, for an RSDP on
/// a 16-byte boundary; returns its physical address.
pub fn find_rsdp(area: &[u8], base: u64) -> Option<u64> {
    (0..area.len().saturating_sub(RSDP_V1_LEN))
        .step_by(16)
        .find(|&off| parse_rsdp(&area[off..]).is_some())
        .map(|off| base + off as u64)
}

/// Reads a whole table at `pa`, checking its length and checksum.
fn read_table<'a>(pa: u64, read: PhysRead<'a>) -> Option<&'a [u8]> {
    let len = read_u32(read(pa, SDT_HEADER_LEN)?, 4)? as usize;
    if len < SDT_HEADER_LEN {
        return None;
    }
    let t = read(pa, len)?;
    checksum_ok(t).then_some(t)
}

/// Finds the table with signature `sig` (e.g. `b"APIC"`).
pub fn find_table<'a>(rsdp: &Rsdp, sig: &[u8; 4], read: PhysRead<'a>) -> Option<&'a [u8]> {
    let (root, entry_size) = if rsdp.revision >= 2 && rsdp.xsdt != 0 {
        (rsdp.xsdt, 8)
    } else {
        (u64::from(rsdp.rsdt), 4)
    };
    let root = read_table(root, read)?;
    root[SDT_HEADER_LEN..]
        .chunks_exact(entry_size)
        .find_map(|e| {
            let pa = if entry_size == 8 {
                read_u64(e, 0)?
            } else {
                u64::from(read_u32(e, 0)?)
            };
            let hdr = read(pa, SDT_HEADER_LEN)?;
            if hdr.get(..4)? != sig {
                return None;
            }
            read_table(pa, read)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn fix_checksum(t: &mut [u8], at: usize) {
        t[at] = 0;
        let s = t.iter().fold(0u8, |s, &x| s.wrapping_add(x));
        t[at] = 0u8.wrapping_sub(s);
    }

    fn table(sig: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut t = vec![0u8; SDT_HEADER_LEN];
        t[..4].copy_from_slice(sig);
        t.extend_from_slice(body);
        let len = t.len() as u32;
        t[4..8].copy_from_slice(&len.to_le_bytes());
        fix_checksum(&mut t, 9);
        t
    }

    /// A tiny physical memory: RSDP (v2) -> XSDT -> two tables.
    fn memory() -> BTreeMap<u64, Vec<u8>> {
        let srat = table(b"SRAT", &[1, 2, 3, 4]);
        let hpet = table(b"HPET", &[0; 20]);
        let mut xsdt_body = Vec::new();
        xsdt_body.extend(0x3000u64.to_le_bytes());
        xsdt_body.extend(0x2000u64.to_le_bytes());
        let xsdt = table(b"XSDT", &xsdt_body);
        let mut rsdp = vec![0u8; 36];
        rsdp[..8].copy_from_slice(RSDP_SIG);
        rsdp[15] = 2;
        rsdp[20..24].copy_from_slice(&36u32.to_le_bytes());
        rsdp[24..32].copy_from_slice(&0x1000u64.to_le_bytes());
        fix_checksum(&mut rsdp[..RSDP_V1_LEN], 8); // ACPI 1.0 part
        fix_checksum(&mut rsdp, 32); // extended checksum, whole RSDP
        BTreeMap::from([
            (0xf2400, rsdp),
            (0x1000, xsdt),
            (0x2000, srat),
            (0x3000, hpet),
        ])
    }

    #[test]
    fn finds_rsdp_and_tables() {
        let mem = memory();
        let read = |pa: u64, len: usize| mem.get(&pa).and_then(|t| t.get(..len));
        let mut area = vec![0u8; 0x1000];
        area[0x400..0x400 + 36].copy_from_slice(&mem[&0xf2400]);
        assert_eq!(find_rsdp(&area, 0xf2000), Some(0xf2400));
        let rsdp = parse_rsdp(&mem[&0xf2400]).unwrap();
        assert_eq!((rsdp.revision, rsdp.xsdt), (2, 0x1000));
        let srat = find_table(&rsdp, b"SRAT", &read).unwrap();
        assert_eq!(&srat[SDT_HEADER_LEN..], [1, 2, 3, 4]);
        assert!(find_table(&rsdp, b"FACP", &read).is_none());
    }

    #[test]
    fn rejects_bad_checksums_and_truncation() {
        let mut mem = memory();
        mem.get_mut(&0x2000).unwrap()[37] ^= 1;
        let read = |pa: u64, len: usize| mem.get(&pa).and_then(|t| t.get(..len));
        let rsdp = parse_rsdp(&mem[&0xf2400]).unwrap();
        assert!(find_table(&rsdp, b"SRAT", &read).is_none());
        let mut bad = mem[&0xf2400].clone();
        bad[0] = b'X';
        assert!(parse_rsdp(&bad).is_none());
        assert!(parse_rsdp(&bad[..10]).is_none());
    }
}
