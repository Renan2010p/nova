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

//! CPU identification (FreeBSD's `x86/x86/identcpu.c`): the CPUID feature
//! bits the kernel decides on, and the boot-time report, printed exactly
//! like FreeBSD's `printcpuinfo` so that a Renux "Nova" dmesg reads like a
//! FreeBSD one:
//!
//! ```text
//! CPU: Intel(R) Core(TM) i7-8750H CPU @ 2.20GHz (2208.00-MHz K8-class CPU)
//!   Origin="GenuineIntel"  Id=0x906ea  Family=0x6  Model=0x9e  Stepping=10
//!   Features=0x1783fbff<FPU,VME,DE,PSE,TSC,MSR,PAE,MCE,CX8,APIC,...>
//!   ...
//! Hypervisor: Origin = "bhyve bhyve "
//! ```
//!
//! The feature names are FreeBSD's (the `%b` strings of `printcpuinfo`).

use core::fmt::{self, Write};

use crate::cpufunc::cpuid;
use crate::specialreg::{AMDID_PAGE1GB, CPUID2_HV, CPUID2_X2APIC};

/// `cpuid(1).ecx` (FreeBSD's `cpu_feature2`).
pub fn cpu_feature2() -> u32 {
    cpuid(1, 0)[2]
}

/// Highest extended leaf, or 0.
fn cpu_exthigh() -> u32 {
    let h = cpuid(0x8000_0000, 0)[0];
    if h >= 0x8000_0000 { h } else { 0 }
}

/// `cpuid(0x80000001).edx` (FreeBSD's `amd_feature`), or 0 if the
/// extended leaf does not exist.
pub fn amd_feature() -> u32 {
    if cpu_exthigh() >= 0x8000_0001 {
        cpuid(0x8000_0001, 0)[3]
    } else {
        0
    }
}

/// The CPU supports x2APIC mode.
pub fn has_x2apic() -> bool {
    cpu_feature2() & CPUID2_X2APIC != 0
}

/// The CPU supports 1 GiB pages.
pub fn has_page1gb() -> bool {
    amd_feature() & AMDID_PAGE1GB != 0
}

/// Running under a hypervisor (FreeBSD's `vm_guest != VM_GUEST_NO`).
pub fn vm_guest() -> bool {
    cpu_feature2() & CPUID2_HV != 0
}

/// Bit positions of the SMT and core fields in x2APIC ids, from CPUID
/// leaf 0xB (FreeBSD's `topo_probe_0xb`): `(smt_shift, core_shift)`, so
/// that `id >> core_shift` is the package and `id >> smt_shift` the core.
/// `None` if the leaf does not exist.
pub fn topo_shifts() -> Option<(u32, u32)> {
    if cpuid(0, 0)[0] < 0xb {
        return None;
    }
    let (mut smt, mut core) = (None, None);
    for level in 0..8 {
        let [a, _, c, _] = cpuid(0xb, level);
        match (c >> 8) & 0xff {
            0 => break,
            1 => smt = Some(a & 0x1f),
            2 => core = Some(a & 0x1f),
            _ => {}
        }
    }
    let smt = smt.unwrap_or(0);
    Some((smt, core.unwrap_or(smt)))
}

/// Twelve bytes of a CPUID vendor string, from three registers.
fn vendor_string(regs: [u32; 3], buf: &mut [u8; 12]) -> &str {
    for (i, r) in regs.iter().enumerate() {
        buf[i * 4..i * 4 + 4].copy_from_slice(&r.to_le_bytes());
    }
    core::str::from_utf8(buf).unwrap_or("?")
}

/// FreeBSD's `%b`: `0x<hex><NAME,NAME>`, names of set bits only.
fn write_bits(w: &mut dyn Write, title: &str, value: u32, names: &[&str; 32]) -> fmt::Result {
    write!(w, "\n  {title}=0x{value:x}")?;
    let mut first = true;
    for (bit, name) in names.iter().enumerate() {
        if value & (1 << bit) != 0 && !name.is_empty() {
            w.write_str(if first { "<" } else { "," })?;
            w.write_str(name)?;
            first = false;
        }
    }
    if !first {
        w.write_str(">")?;
    }
    Ok(())
}

/// `CPUID_TO_FAMILY`.
fn family(id: u32) -> u32 {
    let f = (id >> 8) & 0xf;
    if f == 0xf { f + ((id >> 20) & 0xff) } else { f }
}

/// `CPUID_TO_MODEL`.
fn model(id: u32) -> u32 {
    let m = (id >> 4) & 0xf;
    if matches!(family(id), 0x6 | 0xf..) {
        m | ((id >> 12) & 0xf0)
    } else {
        m
    }
}

/// `AMDPM_TSC_INVARIANT` in `cpuid(0x80000007).edx`.
const AMDPM_TSC_INVARIANT: u32 = 0x100;

/// Writes FreeBSD's `printcpuinfo` report for this CPU; `tsc_freq` is the
/// measured TSC frequency in Hz (0 if unknown).
pub fn print_cpu_info(w: &mut dyn Write, tsc_freq: u64) -> fmt::Result {
    let [high, vb, vc, vd] = cpuid(0, 0);
    let mut vbuf = [0u8; 12];
    let vendor = vendor_string([vb, vd, vc], &mut vbuf);

    let exthigh = cpu_exthigh();
    let mut brand = [0u8; 48];
    if exthigh >= 0x8000_0004 {
        for (i, leaf) in (0x8000_0002..=0x8000_0004).enumerate() {
            for (j, r) in cpuid(leaf, 0).iter().enumerate() {
                brand[i * 16 + j * 4..i * 16 + j * 4 + 4].copy_from_slice(&r.to_le_bytes());
            }
        }
    }
    let end = brand.iter().position(|&b| b == 0).unwrap_or(brand.len());
    let model_name = core::str::from_utf8(&brand[..end])
        .unwrap_or("")
        .trim_start_matches(' ');

    write!(
        w,
        "CPU: {} (",
        if model_name.is_empty() {
            "Unknown"
        } else {
            model_name
        }
    )?;
    if tsc_freq != 0 {
        write!(
            w,
            "{}.{:02}-MHz ",
            (tsc_freq + 4999) / 1_000_000,
            ((tsc_freq + 4999) / 10_000) % 100
        )?;
    }
    w.write_str("K8-class CPU)\n")?;

    let [id, _, f2, f1] = if high >= 1 { cpuid(1, 0) } else { [0; 4] };
    write!(w, "  Origin=\"{vendor}\"")?;
    if id != 0 {
        write!(w, "  Id=0x{id:x}")?;
    }
    if matches!(
        vendor,
        "GenuineIntel" | "AuthenticAMD" | "HygonGenuine" | "CentaurHauls"
    ) {
        write!(
            w,
            "  Family=0x{:x}  Model=0x{:x}  Stepping={}",
            family(id),
            model(id),
            id & 0xf
        )?;
    }
    if high >= 1 {
        write_bits(w, "Features", f1, &FEATURES)?;
        if f2 != 0 {
            write_bits(w, "Features2", f2, &FEATURES2)?;
        }
    }
    if exthigh >= 0x8000_0001 {
        let [_, _, a2, a1] = cpuid(0x8000_0001, 0);
        // Bits AMD duplicates from leaf 1 are not repeated (FreeBSD does
        // the same masking in identify_cpu).
        let a1 = a1 & !(f1 & 0x0183_f3ff);
        if a1 != 0 {
            write_bits(w, "AMD Features", a1, &AMD_FEATURES)?;
        }
        if a2 != 0 {
            write_bits(w, "AMD Features2", a2, &AMD_FEATURES2)?;
        }
    }
    if high >= 7 {
        let [_, s1, s2, s3] = cpuid(7, 0);
        if s1 != 0 {
            write_bits(w, "Structured Extended Features", s1, &STDEXT_FEATURES)?;
        }
        if s2 != 0 {
            write_bits(w, "Structured Extended Features2", s2, &STDEXT_FEATURES2)?;
        }
        if s3 != 0 {
            write_bits(w, "Structured Extended Features3", s3, &STDEXT_FEATURES3)?;
        }
    }
    if exthigh >= 0x8000_0007 && cpuid(0x8000_0007, 0)[3] & AMDPM_TSC_INVARIANT != 0 {
        w.write_str("\n  TSC: P-state invariant")?;
    }
    w.write_str("\n")?;

    if f2 & CPUID2_HV != 0 {
        let [_, hb, hc, hd] = cpuid(0x4000_0000, 0);
        let mut hbuf = [0u8; 12];
        writeln!(
            w,
            "Hypervisor: Origin = \"{}\"",
            vendor_string([hb, hc, hd], &mut hbuf)
        )?;
    }
    Ok(())
}

/// `Features=` bit names; "" bits are not printed.
const FEATURES: [&str; 32] = [
    "FPU", "VME", "DE", "PSE", "TSC", "MSR", "PAE", "MCE", "CX8", "APIC", "oldMTRR", "SEP", "MTRR",
    "PGE", "MCA", "CMOV", "PAT", "PSE36", "PN", "CLFLUSH", "<b20>", "DTS", "ACPI", "MMX", "FXSR",
    "SSE", "SSE2", "SS", "HTT", "TM", "IA64", "PBE",
];

/// `Features2=` bit names; "" bits are not printed.
const FEATURES2: [&str; 32] = [
    "SSE3",
    "PCLMULQDQ",
    "DTES64",
    "MON",
    "DS_CPL",
    "VMX",
    "SMX",
    "EST",
    "TM2",
    "SSSE3",
    "CNXT-ID",
    "SDBG",
    "FMA",
    "CX16",
    "xTPR",
    "PDCM",
    "<b16>",
    "PCID",
    "DCA",
    "SSE4.1",
    "SSE4.2",
    "x2APIC",
    "MOVBE",
    "POPCNT",
    "TSCDLT",
    "AESNI",
    "XSAVE",
    "OSXSAVE",
    "AVX",
    "F16C",
    "RDRAND",
    "HV",
];

/// `AMD Features=` bit names; "" bits are not printed.
const AMD_FEATURES: [&str; 32] = [
    "<s0>", "<s1>", "<s2>", "<s3>", "<s4>", "<s5>", "<s6>", "<s7>", "<s8>", "<s9>", "<b10>",
    "SYSCALL", "<s12>", "<s13>", "<s14>", "<s15>", "<s16>", "<s17>", "<b18>", "MP", "NX", "<b21>",
    "MMX+", "<s23>", "<s24>", "FFXSR", "Page1GB", "RDTSCP", "<b28>", "LM", "3DNow!+", "3DNow!",
];

/// `AMD Features2=` bit names; "" bits are not printed.
const AMD_FEATURES2: [&str; 32] = [
    "LAHF", "CMP", "SVM", "ExtAPIC", "CR8", "ABM", "SSE4A", "MAS", "Prefetch", "OSVW", "IBS",
    "XOP", "SKINIT", "WDT", "<b14>", "LWP", "FMA4", "TCE", "<b18>", "NodeId", "<b20>", "TBM",
    "Topology", "PCXC", "PNXC", "<b25>", "DBE", "PTSC", "PL2I", "MWAITX", "ADMSKX", "<b31>",
];

/// `Structured Extended Features=` bit names; "" bits are not printed.
const STDEXT_FEATURES: [&str; 32] = [
    "FSGSBASE",
    "TSCADJ",
    "SGX",
    "BMI1",
    "HLE",
    "AVX2",
    "FDPEXC",
    "SMEP",
    "BMI2",
    "ERMS",
    "INVPCID",
    "RTM",
    "PQM",
    "NFPUSG",
    "MPX",
    "PQE",
    "AVX512F",
    "AVX512DQ",
    "RDSEED",
    "ADX",
    "SMAP",
    "AVX512IFMA",
    "<b22>",
    "CLFLUSHOPT",
    "CLWB",
    "PROCTRACE",
    "AVX512PF",
    "AVX512ER",
    "AVX512CD",
    "SHA",
    "AVX512BW",
    "AVX512VL",
];

/// `Structured Extended Features2=` bit names; "" bits are not printed.
const STDEXT_FEATURES2: [&str; 32] = [
    "PREFETCHWT1",
    "AVX512VBMI",
    "UMIP",
    "PKU",
    "OSPKE",
    "WAITPKG",
    "AVX512VBMI2",
    "",
    "GFNI",
    "VAES",
    "VPCLMULQDQ",
    "AVX512VNNI",
    "AVX512BITALG",
    "TME",
    "AVX512VPOPCNTDQ",
    "",
    "LA57",
    "",
    "",
    "",
    "",
    "",
    "RDPID",
    "",
    "",
    "CLDEMOTE",
    "",
    "MOVDIRI",
    "MOVDIR64B",
    "ENQCMD",
    "SGXLC",
    "",
];

/// `Structured Extended Features3=` bit names; "" bits are not printed.
const STDEXT_FEATURES3: [&str; 32] = [
    "",
    "",
    "AVX512_4VNNIW",
    "AVX512_4FMAPS",
    "FSRM",
    "",
    "",
    "",
    "AVX512VP2INTERSECT",
    "MCUOPT",
    "MD_CLEAR",
    "",
    "",
    "TSXFA",
    "",
    "",
    "",
    "",
    "PCONFIG",
    "",
    "IBT",
    "",
    "",
    "",
    "",
    "",
    "IBPB",
    "STIBP",
    "L1DFL",
    "ARCH_CAP",
    "CORE_CAP",
    "SSBD",
];
