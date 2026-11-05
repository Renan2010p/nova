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

//! `freebsd_amd64_efi`: booting a kernel by FreeBSD's protocol on amd64
//! from UEFI (FreeBSD's `stand/efi/loader/arch/amd64`: `elf64_freebsd.c`,
//! `amd64_tramp.S`, and `bootinfo.c`'s `bi_load`): [`boot`].
//!
//! The kernel is staged anywhere the firmware gives memory, and must run
//! at physical 2 MiB, mapped at `KERNBASE`. After `ExitBootServices`, a
//! trampoline in a page of its own copies the staging area into place,
//! pushes what `btext` reads (`modulep` and `kernend`), loads page tables
//! that map every GiB of virtual space onto the first GiB of physical
//! memory (FreeBSD's "brutal" mapping, enough until the kernel builds its
//! own) and jumps to the entry point.

#![no_std]
#![deny(unsafe_code)]

mod boot;

pub use boot::{Fail, boot};

use efi::{Error, Pages};
use libkern::conv::{u64_from_usize, usize_from_u64};

/// `MACHINE_ARCH`, for `MODINFOMD_EFI_ARCH`.
pub const MACHINE_ARCH: &str = "amd64";

/// x86 preload metadata of the EFI loader (`x86/include/metadata.h`; the
/// kernel's side is `sys/arch/amd64/metadata.rs`).
pub const MODINFOMD_EFI_MAP: u32 = 0x1004;
/// See [`MODINFOMD_EFI_MAP`].
pub const MODINFOMD_EFI_FB: u32 = 0x1005;
/// See [`MODINFOMD_EFI_MAP`].
pub const MODINFOMD_MODULEP: u32 = 0x1006;
/// See [`MODINFOMD_EFI_MAP`].
pub const MODINFOMD_EFI_ARCH: u32 = 0x1008;

/// The offset from a kernel virtual address to the physical address it
/// loads at (`__elfN(relocation_offset)`): the kernel is linked at
/// `KERNBASE` plus its physical address.
pub fn virt_to_phys(entry: u64) -> impl Fn(u64) -> u64 {
    let base = entry & 0xffff_ffff_ff00_0000;
    move |va| va.wrapping_sub(base)
}

/// What the trampoline reads, in its page at `PARAMS_OFF` (offsets used by
/// the assembly below).
#[repr(C)]
struct Params {
    src: u64,
    len: u64,
    dst: u64,
    modulep: u64,
    kernend: u64,
    pt4: u64,
    entry: u64,
    stack: u64,
}

const PARAMS_OFF: usize = 0x800;
const PAGE: usize = 4096;

/// The trampoline: position-independent, copied into its own page.
mod tramp {
    #![allow(unsafe_code)]

    use core::arch::global_asm;

    global_asm!(
        ".globl nova_tramp_start",
        "nova_tramp_start:",
        "    cli",
        "    mov rsp, [rdi + 56]",
        "    mov rsi, [rdi + 0]",
        "    mov rcx, [rdi + 8]",
        "    mov r8, [rdi + 24]",
        "    mov r9, [rdi + 32]",
        "    mov r10, [rdi + 40]",
        "    mov r11, [rdi + 48]",
        "    mov rdi, [rdi + 16]",
        "    cld",
        "    rep movsb",          // the staging area into place
        "    push r9",            // kernend
        "    shl r8, 32",
        "    push r8",            // modulep, as btext reads it at 4(%rsp)
        "    push r11",           // the entry point, for ret
        "    mov cr3, r10",
        "    ret",
        ".globl nova_tramp_end",
            "nova_tramp_end:",
    );
}

#[allow(unsafe_code)]
unsafe extern "C" {
    static nova_tramp_start: u8;
    static nova_tramp_end: u8;
}

/// The trampoline and its page tables, allocated before the boot services
/// end (below 1 GiB, where its own mapping keeps them reachable).
pub struct Trampoline {
    page: Pages,
    pt: Pages,
}

impl Trampoline {
    /// Allocates the trampoline's page and page tables.
    pub fn prepare() -> Result<Trampoline, Error> {
        const G1: u64 = 1 << 30;
        let page = Pages::alloc(1, Some(G1 - 1))?;
        let mut pt = Pages::alloc(3, Some(G1 - 1))?;
        // PML4 -> one PDP -> one PD of 2 MiB pages over the first GiB; every
        // PML4 and PDP slot points at the same next level.
        const V: u64 = 0x1;
        const RW: u64 = 0x2;
        const PS: u64 = 0x80;
        let base = pt.addr();
        let tables = pt.bytes_mut();
        for i in 0..512usize {
            let pml4e = (base + 4096) | V | RW;
            let pdpe = (base + 8192) | V | RW;
            let pde = (u64_from_usize(i) << 21) | V | RW | PS;
            tables[8 * i..8 * i + 8].copy_from_slice(&pml4e.to_le_bytes());
            tables[4096 + 8 * i..4096 + 8 * i + 8].copy_from_slice(&pdpe.to_le_bytes());
            tables[8192 + 8 * i..8192 + 8 * i + 8].copy_from_slice(&pde.to_le_bytes());
        }
        Ok(Trampoline { page, pt })
    }

    /// The physical range the trampoline uses, which the kernel's copy
    /// must not overwrite.
    pub fn ranges(&self) -> [(u64, u64); 2] {
        [
            (self.page.addr(), self.page.addr() + u64_from_usize(self.page.len())),
            (self.pt.addr(), self.pt.addr() + u64_from_usize(self.pt.len())),
        ]
    }

    /// Copies `len` bytes from `src` to `dst`, then enters the kernel at
    /// `entry` with `modulep` and `kernend`. Called after the boot services
    /// ended; never returns.
    pub fn enter(mut self, src: u64, len: u64, dst: u64, modulep: u64, kernend: u64, entry: u64) -> ! {
        #[allow(unsafe_code)]
        // SAFETY: the two symbols delimit the trampoline's code above; it
        // is position-independent, and shorter than `PARAMS_OFF` (checked).
        let code = unsafe {
            let start = core::ptr::addr_of!(nova_tramp_start);
            let end = core::ptr::addr_of!(nova_tramp_end);
            core::slice::from_raw_parts(start, end.addr() - start.addr())
        };
        assert!(code.len() < PARAMS_OFF, "trampoline too large");
        let params = Params {
            src,
            len,
            dst,
            modulep,
            kernend,
            pt4: self.pt.addr(),
            entry,
            stack: self.page.addr() + u64_from_usize(PAGE) - 16,
        };
        let page = self.page.bytes_mut();
        page[..code.len()].copy_from_slice(code);
        for (i, v) in [params.src, params.len, params.dst, params.modulep, params.kernend, params.pt4, params.entry, params.stack]
            .iter()
            .enumerate()
        {
            page[PARAMS_OFF + 8 * i..PARAMS_OFF + 8 * i + 8].copy_from_slice(&v.to_le_bytes());
        }
        let code_addr = usize_from_u64(self.page.addr());
        let params_addr = usize_from_u64(self.page.addr()) + PARAMS_OFF;
        #[allow(unsafe_code)]
        // SAFETY: the page holds the trampoline and its parameters; the boot
        // services are gone, so nothing else uses this memory; the
        // trampoline never returns (it jumps into the kernel).
        unsafe {
            core::arch::asm!("jmp {code}", code = in(reg) code_addr, in("rdi") params_addr, options(noreturn));
        }
    }
}
