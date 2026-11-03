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

//! GDT, TSS and IDT (FreeBSD's `x86/include/segments.h` for amd64 and the
//! descriptor setup in `amd64/machdep.c`).
//!
//! The GDT layout is FreeBSD's, slot for slot. It is part of the userland
//! ABI: selectors such as `%cs = 0x43` appear in signal contexts and are
//! checked by `sigreturn`, and `syscall`/`sysret` require the user and
//! kernel segments in exactly this order.
//!
//! SMP: every CPU has its own GDT and TSS (the TSS holds per-CPU stacks).
//! The IDT is shared by all CPUs, as in FreeBSD.

use core::cell::UnsafeCell;
use core::mem::size_of;

use x86::cpufunc::{RegionDescriptor, lgdt, lidt, ltr};
use x86::segments::{
    SD_KCODE64, SD_KDATA, SD_UCODE32, SD_UCODE64, SD_UDATA, SDT_SYSIGT, SEL_KPL, gate_descriptor,
    gsel, tss_descriptor,
};

use crate::param::{MAXCPU, PAGE_SIZE};

/// Null descriptor.
pub const GNULL_SEL: usize = 0;
/// Null descriptor.
pub const GNULL2_SEL: usize = 1;
/// User 32-bit `%fs`.
pub const GUFS32_SEL: usize = 2;
/// User 32-bit `%gs`.
pub const GUGS32_SEL: usize = 3;
/// Kernel code.
pub const GCODE_SEL: usize = 4;
/// Kernel data.
pub const GDATA_SEL: usize = 5;
/// User 32-bit code.
pub const GUCODE32_SEL: usize = 6;
/// User 32/64-bit data.
pub const GUDATA_SEL: usize = 7;
/// User 64-bit code.
pub const GUCODE_SEL: usize = 8;
/// TSS (two slots: 9 and 10).
pub const GPROC0_SEL: usize = 9;
/// User LDT (two slots: 11 and 12).
pub const GUSERLDT_SEL: usize = 11;
/// Number of GDT slots.
pub const NGDT: usize = 13;

/// IST slot of the double fault stack (FreeBSD: ist1).
pub const IST_DBLFAULT: u8 = 1;
/// IST slot of the NMI stack (FreeBSD: ist2).
pub const IST_NMI: u8 = 2;
/// IST slot of the machine check stack (FreeBSD: ist3).
pub const IST_MCHK: u8 = 3;

/// Size of each IST stack.
const IST_STACK_SIZE: usize = 4 * PAGE_SIZE;

/// The 64-bit task state segment (FreeBSD's `struct amd64tss`).
#[repr(C, packed(4))]
struct AmdTss {
    _rsvd0: u32,
    rsp: [u64; 3],
    _rsvd1: u64,
    ist: [u64; 7],
    _rsvd2: u64,
    _rsvd3: u16,
    iobase: u16,
}

const _: () = assert!(size_of::<AmdTss>() == 104);

impl AmdTss {
    const EMPTY: Self = Self {
        _rsvd0: 0,
        rsp: [0; 3],
        _rsvd1: 0,
        ist: [0; 7],
        _rsvd2: 0,
        _rsvd3: 0,
        // No I/O permission bitmap: user mode never gets port access.
        iobase: size_of::<AmdTss>() as u16,
    };
}

/// One `T` per CPU, each written only by its own CPU, during that CPU's
/// bring-up.
struct PerCpu<T>([UnsafeCell<T>; MAXCPU]);

// SAFETY: slot `n` is only accessed by CPU `n` (`cpu_tables_init(n)` runs
// on CPU n), and after that only by the hardware.
unsafe impl<T> Sync for PerCpu<T> {}

static GDT: PerCpu<[u64; NGDT]> = PerCpu([const { UnsafeCell::new([0; NGDT]) }; MAXCPU]);
static COMMON_TSS: PerCpu<AmdTss> = PerCpu([const { UnsafeCell::new(AmdTss::EMPTY) }; MAXCPU]);

/// The IDT: 256 gates of two words.
struct Idt(UnsafeCell<[[u64; 2]; 256]>);

// SAFETY: written only by the boot CPU in `idt_init`, before interrupts
// are enabled and before any other CPU starts; read-only afterwards.
unsafe impl Sync for Idt {}

static IDT: Idt = Idt(UnsafeCell::new([[0; 2]; 256]));

/// A stack used only by the hardware, through the TSS.
#[repr(C, align(16))]
struct IstStack(UnsafeCell<[u8; IST_STACK_SIZE]>);

// SAFETY: only the CPU's interrupt machinery touches it, never Rust code.
unsafe impl Sync for IstStack {}

impl IstStack {
    const fn new() -> Self {
        Self(UnsafeCell::new([0; IST_STACK_SIZE]))
    }

    fn top(&self) -> u64 {
        self.0.get() as u64 + IST_STACK_SIZE as u64
    }
}

// TODO(smp): the APs get their IST stacks from the page allocator.
static BOOT_DBLFAULT_STACK: IstStack = IstStack::new();
static BOOT_NMI_STACK: IstStack = IstStack::new();
static BOOT_MCHK_STACK: IstStack = IstStack::new();

/// Installs gate `vec` (FreeBSD's `setidt`).
fn setidt(vec: u8, func: usize, ist: u8) {
    let gate = gate_descriptor(
        func as u64,
        gsel(GCODE_SEL, SEL_KPL),
        ist,
        SDT_SYSIGT,
        SEL_KPL,
    );
    // SAFETY: only called from `idt_init`, see `Idt`'s Sync impl.
    unsafe { (*IDT.0.get())[vec as usize] = gate };
}

/// Fills the shared IDT. Boot CPU only, once, before interrupts are on.
pub(crate) fn idt_init() {
    use crate::exception::*;
    use crate::trap::{APIC_SPURIOUS_INT, APIC_TIMER_INT};

    let traps: [(u8, unsafe extern "C" fn(), u8); 20] = [
        (0, Xdiv, 0),
        (1, Xdbg, 0),
        (2, Xnmi, IST_NMI),
        (3, Xbpt, 0),
        (4, Xofl, 0),
        (5, Xbnd, 0),
        (6, Xill, 0),
        (7, Xdna, 0),
        (8, Xdblfault, IST_DBLFAULT),
        (9, Xfpusegm, 0),
        (10, Xtss, 0),
        (11, Xmissing, 0),
        (12, Xstk, 0),
        (13, Xprot, 0),
        (14, Xpage, 0),
        (15, Xrsvd, 0),
        (16, Xfpu, 0),
        (17, Xalign, 0),
        (18, Xmchk, IST_MCHK),
        (19, Xxmm, 0),
    ];
    for (vec, func, ist) in traps {
        setidt(vec, func as usize, ist);
    }
    // Reserved exceptions; 21 (#CP), 29 (#VC) and 30 (#SX) push an error.
    for vec in 20..32 {
        let func = if matches!(vec, 21 | 29 | 30) {
            Xrsvd_err
        } else {
            Xrsvd
        };
        setidt(vec, func as usize, 0);
    }
    let intrs: [(u8, unsafe extern "C" fn()); 2] = [
        (APIC_TIMER_INT, Xtimerint),
        (APIC_SPURIOUS_INT, Xspuriousint),
    ];
    for (vec, func) in intrs {
        setidt(vec, func as usize, 0);
    }
}

/// Sets up and loads CPU `cpuid`'s GDT and TSS, reloads the segment
/// registers, and loads the shared IDT. Each CPU calls it on itself.
pub(crate) fn cpu_tables_init(cpuid: u32) {
    let id = cpuid as usize;
    assert_eq!(id, 0, "TODO(smp): IST stacks for the APs");
    let tss = COMMON_TSS.0[id].get();
    let gdt = GDT.0[id].get();

    let mut ist = [0u64; 7];
    ist[IST_DBLFAULT as usize - 1] = BOOT_DBLFAULT_STACK.top();
    ist[IST_NMI as usize - 1] = BOOT_NMI_STACK.top();
    ist[IST_MCHK as usize - 1] = BOOT_MCHK_STACK.top();
    let [tss_lo, tss_hi] = tss_descriptor(tss as u64, size_of::<AmdTss>() as u64);

    // SAFETY: this CPU owns slot `id` (see `PerCpu`). The GDT entries are
    // FreeBSD's layout; the TSS descriptor points at this CPU's TSS. After
    // `lgdt`, %cs is reloaded with a far return and %ds/%es/%ss with the
    // kernel data selector; %fs and %gs are left alone so that their
    // bases (MSR_GSBASE: the per-CPU area) are preserved.
    unsafe {
        *tss = AmdTss {
            ist,
            ..AmdTss::EMPTY
        };
        let g = &mut *gdt;
        g[GNULL_SEL] = 0;
        g[GNULL2_SEL] = 0;
        g[GUFS32_SEL] = SD_UDATA;
        g[GUGS32_SEL] = SD_UDATA;
        g[GCODE_SEL] = SD_KCODE64;
        g[GDATA_SEL] = SD_KDATA;
        g[GUCODE32_SEL] = SD_UCODE32;
        g[GUDATA_SEL] = SD_UDATA;
        g[GUCODE_SEL] = SD_UCODE64;
        g[GPROC0_SEL] = tss_lo;
        g[GPROC0_SEL + 1] = tss_hi;
        g[GUSERLDT_SEL] = 0;
        g[GUSERLDT_SEL + 1] = 0;

        lgdt(&RegionDescriptor {
            limit: (size_of::<[u64; NGDT]>() - 1) as u16,
            base: gdt as u64,
        });
        core::arch::asm!(
            "push {cs}",
            "lea {tmp}, [rip + 2f]",
            "push {tmp}",
            "retfq",
            "2:",
            "mov ds, {ds:x}",
            "mov es, {ds:x}",
            "mov ss, {ds:x}",
            cs = in(reg) u64::from(gsel(GCODE_SEL, SEL_KPL)),
            ds = in(reg) u64::from(gsel(GDATA_SEL, SEL_KPL)),
            tmp = out(reg) _,
            options(preserves_flags),
        );
        ltr(gsel(GPROC0_SEL, SEL_KPL));
        lidt(&RegionDescriptor {
            limit: (size_of::<[[u64; 2]; 256]>() - 1) as u16,
            base: IDT.0.get() as u64,
        });
    }
}
