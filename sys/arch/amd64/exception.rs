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

//! Trap and interrupt entry points (FreeBSD's `amd64/exception.S`).
//!
//! Each `IDTVEC` stub makes the stack look the same, whatever the vector:
//! it pushes a dummy error code when the CPU does not push one, then the
//! trap number, and jumps to `alltraps`, which saves the general registers
//! and falls into `calltrap`, which calls `trap::trap` with a pointer to
//! the resulting
//! [`TrapFrame`](crate::trap::TrapFrame).
//!
//! The trap numbers are FreeBSD's `T_*` codes, not vector numbers (a page
//! fault is "trap 12"), so that panic messages read the same. Interrupts
//! push `T_INTR | vector` instead.
//!
//! The CPU aligns `%rsp` to 16 bytes before pushing its frame; 22 words
//! later (hardware frame, error, trap number, 15 registers) the stack is
//! still aligned when `trap` is called, as the ABI requires.
//!
//! TODO(user): `swapgs` on entry from and exit to user mode.

use core::arch::global_asm;

use crate::trap::trap;

/// A stub for a vector without a CPU error code.
macro_rules! idtvec {
    ($name:literal, $trapno:expr) => {
        concat!(
            ".balign 16\n",
            ".globl ",
            $name,
            "\n",
            ".type ",
            $name,
            ", @function\n",
            $name,
            ":\n",
            "    push 0\n",
            "    push ",
            $trapno,
            "\n",
            "    jmp alltraps\n",
            ".size ",
            $name,
            ", . - ",
            $name,
            "\n",
        )
    };
}

/// A stub for a vector whose error code the CPU pushes.
macro_rules! idtvec_err {
    ($name:literal, $trapno:expr) => {
        concat!(
            ".balign 16\n",
            ".globl ",
            $name,
            "\n",
            ".type ",
            $name,
            ", @function\n",
            $name,
            ":\n",
            "    push ",
            $trapno,
            "\n",
            "    jmp alltraps\n",
            ".size ",
            $name,
            ", . - ",
            $name,
            "\n",
        )
    };
}

global_asm!(
    ".section .text",
    idtvec!("Xdiv", "18"),         // T_DIVIDE
    idtvec!("Xdbg", "10"),         // T_TRCTRAP
    idtvec!("Xnmi", "19"),         // T_NMI
    idtvec!("Xbpt", "3"),          // T_BPTFLT
    idtvec!("Xofl", "20"),         // T_OFLOW
    idtvec!("Xbnd", "21"),         // T_BOUND
    idtvec!("Xill", "1"),          // T_PRIVINFLT
    idtvec!("Xdna", "22"),         // T_DNA
    idtvec_err!("Xdblfault", "23"), // T_DOUBLEFLT
    idtvec!("Xfpusegm", "24"),     // T_FPOPFLT
    idtvec_err!("Xtss", "25"),     // T_TSSFLT
    idtvec_err!("Xmissing", "26"), // T_SEGNPFLT
    idtvec_err!("Xstk", "27"),     // T_STKFLT
    idtvec_err!("Xprot", "9"),     // T_PROTFLT
    idtvec_err!("Xpage", "12"),    // T_PAGEFLT
    idtvec!("Xrsvd", "30"),        // T_RESERVED
    idtvec_err!("Xrsvd_err", "30"), // T_RESERVED, with error code
    idtvec!("Xfpu", "6"),          // T_ARITHTRAP
    idtvec_err!("Xalign", "14"),   // T_ALIGNFLT
    idtvec!("Xmchk", "28"),        // T_MCHK
    idtvec!("Xxmm", "29"),         // T_XMMFLT
    idtvec!("Xtimerint", "0x1ef"),   // T_INTR | APIC_TIMER_INT
    idtvec!("Xspuriousint", "0x1ff"), // T_INTR | APIC_SPURIOUS_INT
    idtvec!("Xipi_intr_bitmap_handler", "0x1f8"), // T_INTR | IPI_BITMAP_VECTOR
    idtvec!("Xcpustop", "0x1f9"),     // T_INTR | IPI_STOP
    "",
    // Typed and sized, so that backtraces can name them.
    ".globl alltraps",
    ".type alltraps, @function",
    "alltraps:",
    "    push r15",
    "    push r14",
    "    push r13",
    "    push r12",
    "    push r11",
    "    push r10",
    "    push r9",
    "    push r8",
    "    push rbp",
    "    push rdi",
    "    push rsi",
    "    push rdx",
    "    push rcx",
    "    push rbx",
    "    push rax",
    ".size alltraps, . - alltraps",
    // As in FreeBSD, a return address in `calltrap` tells a backtrace
    // that a trap frame lies just above `trap`'s frame.
    ".globl calltrap",
    ".type calltrap, @function",
    "calltrap:",
    "    cld",
    "    mov rdi, rsp",
    "    call {trap}",
    ".globl calltrap_ret",
    "calltrap_ret:",
    "    pop rax",
    "    pop rbx",
    "    pop rcx",
    "    pop rdx",
    "    pop rsi",
    "    pop rdi",
    "    pop rbp",
    "    pop r8",
    "    pop r9",
    "    pop r10",
    "    pop r11",
    "    pop r12",
    "    pop r13",
    "    pop r14",
    "    pop r15",
    "    add rsp, 16", // trap number and error code
    "    iretq",
    ".size calltrap, . - calltrap",
    trap = sym trap,
);

#[allow(missing_docs)]
unsafe extern "C" {
    pub(crate) fn Xdiv();
    pub(crate) fn Xdbg();
    pub(crate) fn Xnmi();
    pub(crate) fn Xbpt();
    pub(crate) fn Xofl();
    pub(crate) fn Xbnd();
    pub(crate) fn Xill();
    pub(crate) fn Xdna();
    pub(crate) fn Xdblfault();
    pub(crate) fn Xfpusegm();
    pub(crate) fn Xtss();
    pub(crate) fn Xmissing();
    pub(crate) fn Xstk();
    pub(crate) fn Xprot();
    pub(crate) fn Xpage();
    pub(crate) fn Xrsvd();
    pub(crate) fn Xrsvd_err();
    pub(crate) fn Xfpu();
    pub(crate) fn Xalign();
    pub(crate) fn Xmchk();
    pub(crate) fn Xxmm();
    pub(crate) fn Xtimerint();
    pub(crate) fn Xspuriousint();
    pub(crate) fn Xipi_intr_bitmap_handler();
    pub(crate) fn Xcpustop();
    /// The return address of `call trap` in `calltrap`.
    pub(crate) fn calltrap_ret();
}
