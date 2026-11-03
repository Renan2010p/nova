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

//! Machine-independent start-up (FreeBSD's `kern/init_main.c`).
//!
//! In FreeBSD, `mi_startup` walks the `SYSINIT` list: VM, malloc, proc0,
//! devices, root mount, and finally `init(8)`. Nova runs the same kind of
//! ordered steps; each one is a function below, in boot order. For now
//! (milestone 2) the kernel reports what it was handed, checks that the
//! heap and the clock work, and halts.

use alloc::boxed::Box;
use alloc::vec::Vec;

use framework::{BootInfo, PAGE_SIZE};
use sys::copyright::{COPYRIGHT, OSTYPE};
use sys::physmem;
use sys::reboot::RB_VERBOSE;

use crate::kern_environment::StaticEnv;
use crate::kern_shutdown::kern_halt;
use crate::{kern_clock, printf, subr_param};

const MIB: u64 = 1024 * 1024;

/// The MI entry point (`mdi::MiStartup`). Called once by the MD bootstrap
/// on the boot CPU, with interrupts disabled; the framework (page
/// allocator, heap) is already initialized.
pub fn mi_startup(boot: &BootInfo<'_>) -> ! {
    let kenv = StaticEnv::new(boot.envp);
    let verbose = boot.howto & RB_VERBOSE != 0;

    print_banner();
    print_boot_info(boot, &kenv, verbose);
    vm_mem_init(boot, verbose);
    kmem_selftest();
    let param = subr_param::init_param1(&kenv);
    clock_start(param.hz);
    print_cpus();
    selftest_trap(&kenv);

    printf!("mi_startup: nothing more to do yet (milestone 2)\n");
    kern_halt()
}

/// The copyright notice and version (`print_caddr_t(copyright)`).
fn print_banner() {
    printf!("{}", COPYRIGHT);
    printf!(
        "{} {} {}\n",
        OSTYPE,
        env!("CARGO_PKG_VERSION"),
        framework::MACHINE
    );
}

/// What the loader handed over.
fn print_boot_info(boot: &BootInfo<'_>, kenv: &StaticEnv<'_>, verbose: bool) {
    printf!("firmware: {}\n", boot.firmware.name());
    printf!(
        "kernel image + boot data: {:#x} - {:#x} ({} KiB)\n",
        boot.kernel.start,
        boot.kernel.end,
        boot.kernel.len() / 1024
    );
    printf!("boothowto: {:#x}\n", boot.howto);

    printf!("physical memory:\n");
    for r in boot.physmem {
        printf!(
            "  {:#014x} - {:#014x} ({} KiB)\n",
            r.start,
            r.end,
            r.len() / 1024
        );
    }
    printf!(
        "real memory  = {} MiB\n",
        physmem::total(boot.physmem) / MIB
    );

    printf!("preloaded files:\n");
    for m in boot.preload.modules() {
        printf!(
            "  {} ({}) at {:#x}, {} bytes\n",
            m.name().unwrap_or("?"),
            m.type_name().unwrap_or("?"),
            m.addr().unwrap_or(0),
            m.size().unwrap_or(0)
        );
    }

    printf!("kernel environment: {} variables\n", kenv.len());
    if verbose {
        for (name, value) in kenv.iter() {
            printf!("  {}={}\n", name, value);
        }
    } else {
        for name in ["kernelname", "console", "smbios.system.product"] {
            if let Some(value) = kenv.getenv(name) {
                printf!("  {}={}\n", name, value);
            }
        }
    }
}

/// Reports the page allocator (`vm_mem_init`).
fn vm_mem_init(boot: &BootInfo<'_>, verbose: bool) {
    if verbose {
        printf!("available physical memory:\n");
        for r in boot.physavail {
            printf!(
                "  {:#014x} - {:#014x} ({} KiB)\n",
                r.start,
                r.end,
                r.len() / 1024
            );
        }
    }
    let free = framework::mm::vm_phys::free_count();
    printf!(
        "avail memory = {} MiB ({} pages)\n",
        free * PAGE_SIZE as u64 / MIB,
        free
    );
}

/// Checks the kernel heap end to end: slab objects, a large block, and
/// that freed memory returns to the page allocator.
fn kmem_selftest() {
    let before = framework::mm::vm_phys::free_count();
    {
        let v: Vec<u64> = (1..=1000).collect();
        let big: Box<[u8]> = alloc::vec![0xa5; 64 * 1024].into_boxed_slice();
        let small: Vec<Box<u32>> = (0..256).map(Box::new).collect();
        assert_eq!(v.iter().sum::<u64>(), 500_500, "kmem selftest: Vec");
        assert!(big.iter().all(|&b| b == 0xa5), "kmem selftest: large block");
        assert!(
            small.iter().zip(0..).all(|(b, i)| **b == i),
            "kmem selftest: slab"
        );
    }
    let after = framework::mm::vm_phys::free_count();
    // Slab pages stay with the heap; the large blocks went back.
    printf!(
        "kmem: Box and Vec work ({} pages kept by the heap's slabs)\n",
        before.saturating_sub(after)
    );
}

/// Starts the clock and checks that it ticks (`initclocks`).
fn clock_start(hz: u32) {
    kern_clock::initclocks(hz);
    framework::cpu::intr_enable();
    let start = kern_clock::ticks();
    while kern_clock::ticks() < start + u64::from(hz) {
        framework::cpu::cpu_idle();
    }
    printf!(
        "clock: hz = {}, {} ticks counted with interrupts on\n",
        hz,
        kern_clock::ticks() - start
    );
}

/// The CPUs (`cpu_mp_announce`).
fn print_cpus() {
    printf!(
        "CPUs: {} running (boot CPU is cpu{}), up to {} supported\n",
        framework::cpu::mp_ncpus(),
        framework::cpu::curcpu().as_u32(),
        framework::MAXCPU
    );
}

/// `debug.nova.selftest=pf` in the loader: take a kernel page fault on
/// purpose, to see that it is caught and reported.
fn selftest_trap(kenv: &StaticEnv<'_>) {
    if kenv.getenv("debug.nova.selftest") == Some("pf") {
        printf!("selftest: taking a page fault on purpose\n");
        framework::cpu::cpu_selftest_fault();
    }
}
