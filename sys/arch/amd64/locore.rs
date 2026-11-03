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

//! `btext`: the very first kernel instruction (FreeBSD's `amd64/locore.S`).
//!
//! The FreeBSD loader(8) (BIOS, EFI, or bhyve's userboot) jumps here with:
//!
//! * the CPU in 64-bit long mode, on a 64-bit code segment;
//! * paging on, physical `[0, 1 GiB)` mapped at `KERNBASE`;
//! * a scratch stack laid out as 32-bit words:
//!
//! ```text
//!   %rsp + 0: return address (unused)
//!   %rsp + 4: modulep  - physical address of the preload metadata
//!   %rsp + 8: kernend  - physical end of kernel + modules + metadata
//! ```
//!
//! `btext` resets `%rflags`, grabs both arguments, switches to a stack the
//! kernel owns and calls `machdep::hammer_time`, which never returns.

use core::arch::global_asm;

use x86::psl::PSL_KERNEL;

use crate::machdep::hammer_time;

/// Size of the boot stack, which later becomes thread0's kernel stack.
const BOOTSTACK_SIZE: usize = 16 * crate::param::PAGE_SIZE;

global_asm!(
    // The linker script puts this section first in the image.
    ".section .text.btext, \"ax\", @progbits",
    ".globl btext",
    ".type btext, @function",
    "btext:",
    // Don't trust what the loader gives for rflags.
    "push {psl_kernel}",
    "popfq",
    // Grab the loader's arguments before leaving its stack.
    "mov rbp, rsp",
    "mov edi, dword ptr [rbp + 4]", // modulep (arg 1)
    "mov esi, dword ptr [rbp + 8]", // kernend (arg 2)
    // Get onto a stack that we can trust; there is no going back now.
    "lea rsp, [rip + bootstack_top]",
    "xor ebp, ebp",
    "call {hammer_time}",
    "ud2",
    ".size btext, . - btext",
    "",
    ".section .bss.bootstack, \"aw\", @nobits",
    ".balign 16",
    "bootstack:",
    ".space {bootstack_size}",
    "bootstack_top:",
    psl_kernel = const PSL_KERNEL,
    bootstack_size = const BOOTSTACK_SIZE,
    hammer_time = sym hammer_time,
);
