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
//! bits the kernel decides on.

use crate::cpufunc::cpuid;
use crate::specialreg::{AMDID_PAGE1GB, CPUID2_HV, CPUID2_X2APIC};

/// `cpuid(1).ecx` (FreeBSD's `cpu_feature2`).
pub fn cpu_feature2() -> u32 {
    cpuid(1, 0)[2]
}

/// `cpuid(0x80000001).edx` (FreeBSD's `amd_feature`), or 0 if the
/// extended leaf does not exist.
pub fn amd_feature() -> u32 {
    if cpuid(0x8000_0000, 0)[0] >= 0x8000_0001 {
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
