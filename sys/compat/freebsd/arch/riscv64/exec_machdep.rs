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

//! FreeBSD/riscv image start-up (`riscv/riscv/exec_machdep.c`,
//! `riscv/include/vmparam.h`).

use framework::user::UserRegs;

/// `USRSTACK` for Sv39: below the shared page at the top of user space.
pub(crate) const USRSTACK: u64 = 0x0000_0040_0000_0000 - 4096;

/// `MAXDSIZ`, `MAXSSIZ` (FreeBSD/riscv's).
pub(crate) const MAXDSIZ: u64 = 1 << 30;
/// See [`MAXDSIZ`].
pub(crate) const MAXSSIZ: u64 = 1 << 30;
/// Where mappings go by default.
pub(crate) const MMAP_BASE: u64 = MAXDSIZ;
/// `ET_DYN_LOAD_ADDR` (FreeBSD/riscv's).
pub(crate) const ET_DYN_LOAD_ADDR: u64 = 0x0010_0000;

/// `pagesizes[]`: 4 KiB, 2 MiB, 1 GiB.
pub(crate) const PAGESIZES: [u64; 3] = [4096, 2 << 20, 1 << 30];

/// `x` index of `sp` (`x2`) and `a0` (`x10`); `x[0]` is `x1`.
pub(crate) const SP: usize = 1;
pub(crate) const A0: usize = 9;

/// `exec_setregs`: `sepc` at the entry, `sp` 16-byte aligned, `a0`
/// pointing at `argc`.
pub(crate) fn exec_setregs(regs: &mut UserRegs, entry: u64, stack: u64) {
    *regs = UserRegs::default();
    regs.sepc = entry;
    regs.x[SP] = stack & !0xf;
    regs.x[A0] = stack;
}
