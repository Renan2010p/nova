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

//! The application processor trampoline (FreeBSD's `amd64/mpboot.S`).
//!
//! A started AP begins in 16-bit real mode at `MPTRAMP_ADDR`, where this
//! code is copied. It loads a tiny GDT, enters protected mode, enables PAE
//! and long mode, loads the trampoline page tables (which identity-map the
//! first 2 MiB and hold the kernel's mappings), turns paging on, and jumps
//! to `init_secondary` in the kernel, on the stack the boot CPU prepared.
//!
//! Unlike FreeBSD, which relocates the code at run time, the trampoline
//! always runs at the same address, so every address in it is computed by
//! the assembler. It is AT&T syntax, like FreeBSD's.

use core::arch::global_asm;

/// Physical address the trampoline runs at; must be page aligned, below
/// 1 MiB, and in RAM the page allocator never hands out. The SIPI vector
/// is `MPTRAMP_ADDR >> 12`.
pub(crate) const MPTRAMP_ADDR: u64 = 0x8000;

global_asm!(
    ".section .rodata.mptramp, \"a\"",
    ".balign 16",
    ".globl mptramp_start",
    "mptramp_start:",
    ".code16",
    "    cli",
    "    movw %cs, %ax",
    "    movw %ax, %ds",
    "    lgdtl mpt_gdtdesc - mptramp_start",
    // PE only: this also clears CD and NW, which INIT leaves set.
    "    movl $0x11, %eax",
    "    movl %eax, %cr0",
    "    ljmpl $0x08, ${addr} + mpt_prot32 - mptramp_start",
    ".code32",
    "mpt_prot32:",
    "    movw $0x10, %ax",
    "    movw %ax, %ds",
    "    movw %ax, %es",
    "    movw %ax, %ss",
    "    movl %cr4, %eax",
    "    orl $0x20, %eax", // CR4_PAE
    "    movl %eax, %cr4",
    "    movl ({addr} + mpt_cr3 - mptramp_start), %eax",
    "    movl %eax, %cr3",
    "    movl $0xc0000080, %ecx", // MSR_EFER
    "    rdmsr",
    "    orl $0x100, %eax", // EFER_LME
    "    wrmsr",
    "    movl $0x80010031, %eax", // CR0_PG | CR0_WP | CR0_NE | CR0_ET | CR0_PE
    "    movl %eax, %cr0",
    "    ljmpl $0x18, ${addr} + mpt_long - mptramp_start",
    ".code64",
    "mpt_long:",
    "    movq ({addr} + mpt_stack - mptramp_start), %rsp",
    "    movq ({addr} + mpt_cpuid - mptramp_start), %rdi",
    "    movq ({addr} + mpt_entry - mptramp_start), %rax",
    "    xorl %ebp, %ebp", // end of backtraces
    "    pushq $0",        // a "return address": the ABI's stack alignment
    "    jmpq *%rax",
    ".balign 8",
    "mpt_gdt:",
    "    .quad 0",
    "    .quad 0x00cf9a000000ffff", // 0x08: 32-bit code
    "    .quad 0x00cf92000000ffff", // 0x10: data
    "    .quad 0x00af9a000000ffff", // 0x18: 64-bit code
    "mpt_gdtdesc:",
    "    .word 31",
    "    .long {addr} + mpt_gdt - mptramp_start",
    ".balign 8",
    ".globl mpt_cr3",
    "mpt_cr3: .quad 0",
    ".globl mpt_stack",
    "mpt_stack: .quad 0",
    ".globl mpt_entry",
    "mpt_entry: .quad 0",
    ".globl mpt_cpuid",
    "mpt_cpuid: .quad 0",
    ".globl mptramp_end",
    "mptramp_end:",
    addr = const MPTRAMP_ADDR,
    options(att_syntax),
);

unsafe extern "C" {
    /// First byte of the trampoline.
    pub(crate) static mptramp_start: u8;
    /// One past the last byte of the trampoline.
    pub(crate) static mptramp_end: u8;
    /// Trampoline parameters, written in the copy before each start.
    pub(crate) static mpt_cr3: u8;
    pub(crate) static mpt_stack: u8;
    pub(crate) static mpt_entry: u8;
    pub(crate) static mpt_cpuid: u8;
}
