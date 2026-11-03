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
//! ordered steps, one function each, below, and prints what FreeBSD
//! prints at each of them, in the same format, so a Renux "Nova" console
//! looks familiar:
//!
//! ```text
//! Copyright (c) 2026 Renan Lucas Vieira Hilário.
//! ...
//! Nova 0.1.0-CURRENT main-n12-446bf2d3e1a2 GENERIC amd64
//! rustc 1.99.1 (58cf8f9c3 2026-10-13)
//! CPU: Intel(R) Core(TM) i7-8750H CPU @ 2.20GHz (2208.00-MHz K8-class CPU)
//!   Origin="GenuineIntel"  Id=0x906ea  Family=0x6  Model=0x9e  Stepping=10
//!   ...
//! real memory  = 536870912 (512 MB)
//! avail memory = 533475328 (508 MB)
//! Event timer "LAPIC" quality 600
//! Timecounters tick every 10.000 msec
//! ```
//!
//! Diagnostics FreeBSD only shows with `boot -v` are only shown with
//! `boot -v` here too. There is no userland yet, so the kernel halts at
//! the point where FreeBSD would start `init(8)`.

use alloc::format;
use alloc::string::String;

use framework::{BootInfo, PAGE_SIZE};
use sys::copyright::{COPYRIGHT, OSTYPE};
use sys::reboot::{RB_HALT, RB_VERBOSE};

use crate::kern_environment::StaticEnv;
use crate::subr_prf::Cons;
use crate::vers::{COMPILER, KERN_IDENT, OSRELEASE, VCS};
use crate::{
    kern_clock, kern_linker, kern_shutdown, printf, sched_4bsd, subr_param, subr_selftest,
};

/// The MI entry point (`mdi::MiStartup`). Called once by the MD bootstrap
/// on the boot CPU, with interrupts disabled; the framework (page
/// allocator, heap, thread 0) is already initialized.
pub fn mi_startup(boot: &BootInfo<'_>) -> ! {
    let kenv = StaticEnv::new(boot.envp);
    let bootverbose = boot.howto & RB_VERBOSE != 0;
    let param = subr_param::init_param1(&kenv);
    kern_shutdown::set_panic_reboot_wait_time(param.panic_reboot_wait_time);
    kern_linker::linker_init(boot);

    print_caddr_t();
    identify_cpu();
    cpu_startup(boot, &kenv, bootverbose);
    if bootverbose {
        linker_preload(boot);
    }
    sched_4bsd::sched_init(param.hz);
    kern_clock::initclocks(param.hz);
    mp_start();
    kern_clock::inittimecounter();
    framework::cpu::intr_enable();
    release_aps(bootverbose);

    subr_selftest::run(bootverbose, param.hz);
    subr_selftest::trap_selftest(&kenv);

    // This is where FreeBSD would start init(8).
    printf!("Nova: no userland yet; halting.\n");
    kern_shutdown::kern_reboot(RB_HALT)
}

/// The copyright notice and version (`print_caddr_t(copyright)`,
/// `print_version`).
fn print_caddr_t() {
    printf!("{}", COPYRIGHT);
    printf!(
        "{} {} {} {} {}\n",
        OSTYPE,
        OSRELEASE,
        VCS,
        KERN_IDENT,
        framework::MACHINE
    );
    printf!("{}\n", COMPILER);
}

/// The CPU report (`printcpuinfo`).
fn identify_cpu() {
    let _ = framework::cpu::print_cpu_info(&mut Cons);
}

/// Memory sizes, as amd64's `cpu_startup` reports them.
fn cpu_startup(boot: &BootInfo<'_>, kenv: &StaticEnv<'_>, bootverbose: bool) {
    let free = framework::mm::vm_phys::free_count() * PAGE_SIZE as u64;
    // SMBIOS knows the installed RAM; the firmware map only what is usable.
    let maxmem = boot.physmem.last().map_or(0, |r| r.end);
    let realmem = kenv
        .getenv("smbios.memory.enabled")
        .and_then(|v| v.parse::<u64>().ok())
        .map(|kib| kib << 10)
        .filter(|&m| m >= free)
        .unwrap_or(maxmem);
    printf!("real memory  = {} ({} MB)\n", realmem, realmem >> 20);
    if bootverbose {
        printf!("Physical memory chunk(s):\n");
        for r in boot.physavail {
            printf!(
                "0x{:016x} - 0x{:016x}, {} bytes ({} pages)\n",
                r.start,
                r.end - 1,
                r.len(),
                r.len() / PAGE_SIZE as u64
            );
        }
    }
    printf!("avail memory = {} ({} MB)\n", free, free / 1_048_576);
}

/// Reports the CPUs the firmware lists (`madt_probe`, `mp_start`,
/// `cpu_mp_announce`). The APs are started later, by `release_aps`.
fn mp_start() {
    if let Some(id) = framework::smp::acpi_madt_ident() {
        let (oem, table) = (
            String::from_utf8_lossy(&id[..6]),
            String::from_utf8_lossy(&id[6..]),
        );
        printf!("ACPI APIC Table: <{} {}>\n", oem, table);
    }
    let ncpus = framework::smp::mp_ncpus_detected();
    if ncpus < 2 {
        return;
    }
    printf!("Nova/SMP: Multiprocessor System Detected: {} CPUs\n", ncpus);
    let (packages, cores, threads) = framework::smp::cpu_topology();
    let mut line = format!("Nova/SMP: {packages} package(s)");
    if cores > 0 {
        line += &format!(" x {cores} core(s)");
    }
    if threads > 1 {
        line += &format!(" x {threads} hardware threads");
    }
    printf!("{}\n", line);
}

/// Starts the application processors (`release_aps`, with the messages
/// of `init_secondary_tail`).
fn release_aps(bootverbose: bool) {
    let ncpus = framework::smp::mp_ncpus_detected();
    for cpu in 1..ncpus {
        if !framework::smp::start_ap(cpu) {
            panic!(
                "AP #{} (PHY# {}) failed!",
                cpu,
                framework::smp::cpu_apic_id(cpu)
            );
        }
        if bootverbose {
            printf!("SMP: AP CPU #{} Launched!\n", cpu);
        } else {
            let prefix = if cpu == 1 { "Launching APs: " } else { "" };
            let suffix = if cpu == ncpus - 1 { "\n" } else { " " };
            printf!("{}{}{}", prefix, cpu, suffix);
        }
    }
}

/// The files the loader preloaded (`linker_preload`, verbose only).
fn linker_preload(boot: &BootInfo<'_>) {
    for m in boot.preload.modules() {
        printf!(
            "Preloaded {} \"{}\" at {:#x}.\n",
            m.type_name().unwrap_or("?"),
            m.name().unwrap_or("?"),
            m.addr().unwrap_or(0)
        );
    }
}
