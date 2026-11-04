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

//! The x86 CPUs of an ACPI MADT (FreeBSD's `x86/acpica/madt.c`): its
//! processor local APIC and local x2APIC entries. Finding the table is
//! machine-independent (`sys::acpi`); what its entries mean is not.

use sys::acpi::SDT_HEADER_LEN;
use sys::bytes::read_u32;

/// MADT signature.
pub const MADT_SIG: &[u8; 4] = b"APIC";
/// Entry: processor local APIC.
const MADT_LOCAL_APIC: u8 = 0;
/// Entry: processor local x2APIC.
const MADT_LOCAL_X2APIC: u8 = 9;
/// CPU flags: enabled (usable now).
const MADT_ENABLED: u32 = 1;
/// Offset of the first entry (header, local APIC address, flags).
const MADT_ENTRIES: usize = SDT_HEADER_LEN + 8;

/// A CPU listed in the MADT.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MadtCpu {
    /// Its local APIC id.
    pub apic_id: u32,
}

/// The enabled CPUs of a MADT, in table order (`madt_add_cpu`). Disabled
/// entries (absent or hot-pluggable CPUs) are skipped, as FreeBSD does.
pub fn madt_cpus(madt: &[u8]) -> impl Iterator<Item = MadtCpu> + '_ {
    let mut off = MADT_ENTRIES;
    core::iter::from_fn(move || {
        loop {
            let kind = *madt.get(off)?;
            let len = *madt.get(off + 1)? as usize;
            if len < 2 {
                return None;
            }
            let e = madt.get(off..off + len)?;
            off += len;
            let cpu = match kind {
                MADT_LOCAL_APIC if len >= 8 => {
                    let apic_id = u32::from(e[3]);
                    (read_u32(e, 4)? & MADT_ENABLED != 0).then_some(MadtCpu { apic_id })
                }
                MADT_LOCAL_X2APIC if len >= 16 => {
                    let apic_id = read_u32(e, 4)?;
                    (read_u32(e, 8)? & MADT_ENABLED != 0).then_some(MadtCpu { apic_id })
                }
                _ => None,
            };
            if cpu.is_some() {
                return cpu;
            }
        }
    })
}

/// The MADT's OEM id and OEM table id, for `ACPI APIC Table: <...>`.
pub fn madt_ident(madt: &[u8]) -> Option<[u8; 14]> {
    madt.get(10..24)?.try_into().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn madt(entries: &[u8]) -> Vec<u8> {
        let mut t = vec![0u8; MADT_ENTRIES];
        t[..4].copy_from_slice(MADT_SIG);
        t[10..16].copy_from_slice(b"BHYVE ");
        t[16..24].copy_from_slice(b"BVAPIC  ");
        t.extend_from_slice(entries);
        t
    }

    #[test]
    fn lists_enabled_cpus_in_order() {
        let mut e = Vec::new();
        e.extend([MADT_LOCAL_APIC, 8, 0, 0, 1, 0, 0, 0]); // apic 0, enabled
        e.extend([MADT_LOCAL_APIC, 8, 1, 2, 0, 0, 0, 0]); // apic 2, disabled
        e.extend([1, 12, 0, 0, 0, 0, 0xc0, 0xfe, 0, 0, 0, 0]); // I/O APIC: ignored
        e.extend([MADT_LOCAL_X2APIC, 16, 0, 0]);
        e.extend(300u32.to_le_bytes());
        e.extend(1u32.to_le_bytes());
        e.extend(7u32.to_le_bytes());
        let t = madt(&e);
        let cpus: Vec<u32> = madt_cpus(&t).map(|c| c.apic_id).collect();
        assert_eq!(cpus, [0, 300]);
        assert_eq!(&madt_ident(&t).unwrap(), b"BHYVE BVAPIC  ");
    }

    #[test]
    fn survives_truncation_and_bad_lengths() {
        let t = madt(&[MADT_LOCAL_APIC, 8, 0, 0, 1, 0, 0, 0, 5, 0]);
        for cut in 0..t.len() {
            let _ = madt_cpus(&t[..cut]).count();
        }
        assert_eq!(madt_cpus(&t).count(), 1); // stops at the zero-length entry
    }
}
