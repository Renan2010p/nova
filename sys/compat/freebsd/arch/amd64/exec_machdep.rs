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

//! FreeBSD/amd64 image start-up (`amd64/amd64/exec_machdep.c`,
//! `amd64/include/vmparam.h`).

use framework::user::UserRegs;

/// `USRSTACK_LA48`: the top of the user stack, below the shared page.
pub(crate) const USRSTACK: u64 = 0x0000_8000_0000_0000 - 4096;

/// `MAXDSIZ`: the largest data segment, and where `mmap` starts placing
/// what has no address (`vm_daddr + maxdsiz`, the program being low).
pub(crate) const MAXDSIZ: u64 = 32 << 30;
/// `MAXSSIZ`: the largest stack.
pub(crate) const MAXSSIZ: u64 = 512 << 20;
/// Where mappings go by default (see [`MAXDSIZ`]).
pub(crate) const MMAP_BASE: u64 = MAXDSIZ;
/// `ET_DYN_LOAD_ADDR`: where a position-independent executable loads.
pub(crate) const ET_DYN_LOAD_ADDR: u64 = 0x0102_1000;

/// `pagesizes[]` (`MAXPAGESIZES` entries): 4 KiB, 2 MiB, 1 GiB.
pub(crate) const PAGESIZES: [u64; 3] = [4096, 2 << 20, 1 << 30];

/// `PSL_USER`: interrupts on, the reserved bit set.
const PSL_USER: u64 = 0x202;

/// `exec_setregs`: a fresh register file entering `entry`, with `%rsp`
/// aligned as after a call and `%rdi` pointing at `argc`.
pub(crate) fn exec_setregs(regs: &mut UserRegs, entry: u64, stack: u64) {
    *regs = UserRegs::default();
    regs.rip = entry;
    regs.rsp = ((stack - 8) & !0xf) + 8;
    regs.rdi = stack;
    regs.rflags = PSL_USER;
}
