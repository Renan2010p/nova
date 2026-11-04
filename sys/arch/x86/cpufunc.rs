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

use libkern::conv::trunc_u32;
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
        asm!("wrmsr", in("ecx") msr, in("eax") trunc_u32(v), in("edx") trunc_u32(v >> 32),
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

/// `sti; hlt`: enables interrupts and sleeps until the next one. The two
/// instructions are adjacent, so an interrupt cannot slip in between and
/// be missed (`sti` takes effect after the next instruction).
#[inline(always)]
pub fn sti_hlt() {
    // SAFETY: as for `sti` and `hlt`.
    unsafe { asm!("sti", "hlt", options(nomem, nostack, preserves_flags)) }
}

/// `cpuid`: returns `[eax, ebx, ecx, edx]` for `leaf`/`subleaf`.
#[inline]
pub fn cpuid(leaf: u32, subleaf: u32) -> [u32; 4] {
    let (eax, ebx, ecx, edx): (u32, u64, u32, u32);
    // SAFETY: cpuid only reads processor information. %rbx is reserved
    // by LLVM, so it is saved in a scratch register around the instruction.
    unsafe {
        asm!(
            "mov {tmp}, rbx",
            "cpuid",
            "xchg {tmp}, rbx",
            tmp = out(reg) ebx,
            inout("eax") leaf => eax,
            inout("ecx") subleaf => ecx,
            out("edx") edx,
            options(nomem, nostack, preserves_flags),
        )
    }
    [eax, trunc_u32(ebx), ecx, edx]
}

/// Reads `%cr2`, the faulting address of the last page fault.
#[inline(always)]
pub fn rcr2() -> u64 {
    let v: u64;
    // SAFETY: reading a control register has no side effects.
    unsafe { asm!("mov {}, cr2", out(reg) v, options(nomem, nostack, preserves_flags)) }
    v
}

/// Reads `%cr3`, the physical address of the current top-level page table.
#[inline(always)]
pub fn rcr3() -> u64 {
    let v: u64;
    // SAFETY: reading a control register has no side effects.
    unsafe { asm!("mov {}, cr3", out(reg) v, options(nomem, nostack, preserves_flags)) }
    v
}

/// Loads `%cr3`: switches to another set of page tables and flushes the
/// non-global TLB entries.
///
/// # Safety
///
/// `pml4` must be the physical address of a valid top-level page table
/// that maps the currently executing code, its stack and everything the
/// kernel will touch, with the same meaning as before.
#[inline(always)]
pub unsafe fn load_cr3(pml4: u64) {
    // SAFETY: forwarded to the caller. No `nomem`: this changes what every
    // address means, so the compiler must not move memory accesses across.
    unsafe { asm!("mov cr3, {}", in(reg) pml4, options(nostack, preserves_flags)) }
}

/// Reads `%cr4`.
#[inline(always)]
pub fn rcr4() -> u64 {
    let v: u64;
    // SAFETY: reading a control register has no side effects.
    unsafe { asm!("mov {}, cr4", out(reg) v, options(nomem, nostack, preserves_flags)) }
    v
}

/// Loads `%cr4`.
///
/// # Safety
///
/// The new value must only enable features the CPU supports and the
/// kernel is prepared for.
#[inline(always)]
pub unsafe fn load_cr4(v: u64) {
    // SAFETY: forwarded to the caller.
    unsafe { asm!("mov cr4, {}", in(reg) v, options(nostack, preserves_flags)) }
}

/// Pointer operand of `lgdt`/`lidt` (FreeBSD's `struct region_descriptor`).
#[repr(C, packed)]
pub struct RegionDescriptor {
    /// Size of the table in bytes, minus one.
    pub limit: u16,
    /// Linear address of the table.
    pub base: u64,
}

/// `lgdt`: loads the global descriptor table register.
///
/// # Safety
///
/// The table must stay valid and unchanged in the entries in use for as
/// long as it is loaded, and the caller must reload the segment registers
/// it wants to take from it.
#[inline(always)]
pub unsafe fn lgdt(rd: &RegionDescriptor) {
    // SAFETY: forwarded to the caller.
    unsafe { asm!("lgdt [{}]", in(reg) rd, options(readonly, nostack, preserves_flags)) }
}

/// `lidt`: loads the interrupt descriptor table register.
///
/// # Safety
///
/// The table must stay valid for as long as it is loaded, and every
/// present gate must point to a correct entry stub.
#[inline(always)]
pub unsafe fn lidt(rd: &RegionDescriptor) {
    // SAFETY: forwarded to the caller.
    unsafe { asm!("lidt [{}]", in(reg) rd, options(readonly, nostack, preserves_flags)) }
}

/// `ltr`: loads the task register.
///
/// # Safety
///
/// `sel` must select an available 64-bit TSS descriptor in the current
/// GDT, describing a TSS that stays valid while loaded.
#[inline(always)]
pub unsafe fn ltr(sel: u16) {
    // SAFETY: forwarded to the caller.
    unsafe { asm!("ltr {0:x}", in(reg) sel, options(nostack, preserves_flags)) }
}

/// `rdtsc`: reads the time-stamp counter.
#[inline(always)]
pub fn rdtsc() -> u64 {
    let (lo, hi): (u32, u32);
    // SAFETY: reads a counter; no side effects.
    unsafe { asm!("rdtsc", out("eax") lo, out("edx") hi, options(nomem, nostack, preserves_flags)) }
    (u64::from(hi) << 32) | u64::from(lo)
}

/// `invlpg`: flushes the TLB entry for `va` on this CPU.
#[inline(always)]
pub fn invlpg(va: usize) {
    // SAFETY: dropping a TLB entry only makes the next access walk the page
    // tables again; it cannot make memory accessible that was not.
    unsafe { asm!("invlpg [{}]", in(reg) va, options(nostack, preserves_flags)) }
}

/// Reads `%cr0`.
#[inline(always)]
pub fn rcr0() -> u64 {
    let v: u64;
    // SAFETY: reading a control register has no side effects.
    unsafe { asm!("mov {}, cr0", out(reg) v, options(nomem, nostack, preserves_flags)) }
    v
}

/// Loads `%cr0`.
///
/// # Safety
///
/// The new value must keep protected mode and paging on, and only change
/// what the kernel is prepared for (caching, write protection).
#[inline(always)]
pub unsafe fn load_cr0(v: u64) {
    // SAFETY: forwarded to the caller.
    unsafe { asm!("mov cr0, {}", in(reg) v, options(nostack, preserves_flags)) }
}
