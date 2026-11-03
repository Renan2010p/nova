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

//! Inline wrappers for single x86 instructions (FreeBSD's
//! `machine/cpufunc.h`).
//!
//! An instruction that cannot violate memory safety by itself is exposed as
//! a safe `fn` (`cli`, `hlt`, `pause`). One that can (port I/O may drive a
//! DMA-capable device) is an `unsafe fn` with its contract documented.

use core::arch::asm;

/// `cli`: disables maskable interrupts on this CPU.
#[inline(always)]
pub fn disable_intr() {
    // SAFETY: clearing IF only delays interrupt delivery; it touches no
    // memory. The asm is also a compiler barrier, which callers rely on.
    unsafe { asm!("cli", options(nostack, preserves_flags)) }
}

/// `sti`: enables maskable interrupts on this CPU.
#[inline(always)]
pub fn enable_intr() {
    // SAFETY: as for `cli`. Whether interrupts may be taken at this point
    // is a correctness concern of the caller, not a memory-safety one,
    // because handlers save and restore all state they clobber.
    unsafe { asm!("sti", options(nostack, preserves_flags)) }
}

/// `hlt`: sleeps until the next interrupt.
#[inline(always)]
pub fn halt() {
    // SAFETY: waits for an interrupt; touches no memory.
    unsafe { asm!("hlt", options(nomem, nostack, preserves_flags)) }
}

/// `pause`: spin-wait hint.
#[inline(always)]
pub fn ia32_pause() {
    // SAFETY: a hint with no architectural effect.
    unsafe { asm!("pause", options(nomem, nostack, preserves_flags)) }
}

/// Reads `%rflags`.
#[inline(always)]
pub fn read_rflags() -> u64 {
    let rflags: u64;
    // SAFETY: pushes and pops one word on the current stack, which is
    // always valid while executing.
    unsafe { asm!("pushfq", "pop {}", out(reg) rflags, options(nomem, preserves_flags)) }
    rflags
}

/// `rdmsr`: reads a model-specific register.
///
/// # Safety
///
/// `msr` must exist on this CPU (otherwise `#GP`), and reading it must
/// have no side effect the caller is not prepared for.
#[inline(always)]
pub unsafe fn rdmsr(msr: u32) -> u64 {
    let (lo, hi): (u32, u32);
    // SAFETY: forwarded to the caller, see `# Safety`.
    unsafe {
        asm!("rdmsr", in("ecx") msr, out("eax") lo, out("edx") hi, options(nomem, nostack, preserves_flags))
    }
    (u64::from(hi) << 32) | u64::from(lo)
}

/// `wrmsr`: writes a model-specific register.
///
/// # Safety
///
/// `msr` must exist on this CPU, and the new value must keep every
/// invariant the kernel relies on (e.g. `MSR_GSBASE` must point to this
/// CPU's per-CPU area).
#[inline(always)]
pub unsafe fn wrmsr(msr: u32, v: u64) {
    // SAFETY: forwarded to the caller, see `# Safety`.
    unsafe {
        asm!("wrmsr", in("ecx") msr, in("eax") v as u32, in("edx") (v >> 32) as u32,
             options(nostack, preserves_flags))
    }
}

/// `inb`: reads one byte from an I/O port.
///
/// # Safety
///
/// Reading a port can have side effects on the device behind it (e.g.
/// acknowledging an interrupt or popping a FIFO). The caller must own the
/// device at `port`.
#[inline(always)]
pub unsafe fn inb(port: u16) -> u8 {
    let v: u8;
    // SAFETY: forwarded to the caller, see `# Safety`.
    unsafe {
        asm!("in al, dx", out("al") v, in("dx") port, options(nomem, nostack, preserves_flags))
    }
    v
}

/// `outb`: writes one byte to an I/O port.
///
/// # Safety
///
/// Writing a port can make a device do anything, including DMA into
/// arbitrary memory. The caller must own the device at `port` and know the
/// effect of the write.
#[inline(always)]
pub unsafe fn outb(port: u16, v: u8) {
    // SAFETY: forwarded to the caller, see `# Safety`.
    unsafe {
        asm!("out dx, al", in("dx") port, in("al") v, options(nomem, nostack, preserves_flags))
    }
}
