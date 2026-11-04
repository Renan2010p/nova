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

//! FreeBSD/amd64 ELF parameters (`amd64/amd64/elf_machdep.c`).

/// `e_machine` of FreeBSD/amd64 binaries (`EM_X86_64`).
pub(crate) const ELF_ARCH: u16 = 62;
/// `sv_name` of the native FreeBSD/amd64 ABI.
pub(crate) const SV_NAME: &str = "FreeBSD ELF64";
