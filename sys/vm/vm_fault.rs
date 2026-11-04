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

//! Page faults from user mode (FreeBSD's `vm/vm_fault.c`): memory that is
//! mapped but not yet present (anonymous memory is allocated and zeroed
//! when first touched) is made present, and the program resumes.

use framework::user::{FaultKind, UserFault, VmSpace};

/// Resolves `fault` in `vm` (`vm_fault_trap`). True if the program may
/// resume; false if the access was not allowed (a signal follows).
pub fn vm_fault_trap(vm: &VmSpace, fault: &UserFault) -> bool {
    let write = match fault.kind {
        FaultKind::NotMapped { write } => write,
        // A present page accessed against its protection: no paging fixes
        // that (until copy-on-write).
        _ => return false,
    };
    vm.fault(fault.addr, write).is_ok()
}
