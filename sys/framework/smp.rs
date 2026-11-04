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

//! Multiprocessor start-up and inter-processor interrupts (the mechanism
//! under FreeBSD's `kern/subr_smp.c` and `x86/x86/mp_x86.c`).
//!
//! The boot CPU starts each application processor with [`start_ap`]: the
//! framework gives it a boot stack, exception stacks and an idle thread, and the
//! MD layer wakes it. The AP sets itself up and calls [`__nova_ap_startup`],
//! which starts its clock and enters the scheduler through its idle thread.
//! From then on it runs threads from the shared run queue.

use alloc::boxed::Box;
use alloc::format;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

use mdi::{Ipi, Smp};

use crate::cpu::{CpuId, curcpu, intr_guard, mp_ncpus_inc};
use crate::mm::kstack::KernelStack;
use crate::sync::MtxSpin;
use crate::task::{self, Task};
use crate::{MAXCPU, Md};

/// CPUs that are running (FreeBSD's `all_cpus` once started).
static ONLINE: [AtomicBool; MAXCPU] = [const { AtomicBool::new(false) }; MAXCPU];
/// CPUs `start_ap` was called for.
static STARTED: [AtomicBool; MAXCPU] = [const { AtomicBool::new(false) }; MAXCPU];

/// The idle thread of the AP being started, handed over to it.
static AP_IDLE: MtxSpin<Option<Arc<Task>>> = MtxSpin::new(None);

/// Marks the boot CPU online. Called by `framework::__init`.
pub(crate) fn init() {
    ONLINE[CpuId::BSP.index()].store(true, Ordering::Release);
}

/// Number of CPUs the firmware lists, the boot CPU included
/// (`cpu_mp_probe`).
pub fn mp_ncpus_detected() -> u32 {
    Md::cpu_mp_probe()
}

/// Packages, cores per package and threads per core (`cpu_mp_announce`).
pub fn cpu_topology() -> (u32, u32, u32) {
    Md::cpu_topology()
}

/// The hardware id of CPU `cpu` (`cpu_hwid`).
pub fn cpu_hwid(cpu: u32) -> u32 {
    Md::cpu_hwid(cpu)
}

/// Writes what the machine reports when it probes the CPUs from firmware
/// tables (FreeBSD's line for that table), if anything.
pub fn cpu_mp_print_probe(w: &mut dyn core::fmt::Write) -> core::fmt::Result {
    Md::cpu_mp_print_probe(w)
}

/// The CPUs that are running.
pub fn online_cpus() -> impl Iterator<Item = CpuId> {
    (0..Md::cpu_mp_probe())
        .filter(|&c| ONLINE[c as usize].load(Ordering::Acquire))
        .map(CpuId::from_u32)
}

/// Starts application processor `cpu` (`start_ap`). Returns false if it
/// did not come up (FreeBSD then panics: "AP #%d (PHY# %d) failed!").
///
/// # Panics
///
/// If `cpu` is the boot CPU, is not in `1..mp_ncpus_detected()`, or was
/// already started.
pub fn start_ap(cpu: u32) -> bool {
    assert!(
        cpu != 0 && cpu < mp_ncpus_detected(),
        "start_ap: no CPU {cpu}"
    );
    assert!(
        !STARTED[cpu as usize].swap(true, Ordering::AcqRel),
        "start_ap: CPU {cpu} already started"
    );

    let name: &'static str = Box::leak(format!("idle: cpu{cpu}").into_boxed_str());
    let idle = task::new_idle(name).expect("start_ap: no memory for the idle thread");
    let boot = KernelStack::new().expect("start_ap: no memory for the boot stack");
    let exception_stacks: Vec<KernelStack> = (0..Md::AP_EXCEPTION_STACKS)
        .map(|_| KernelStack::new().expect("start_ap: no memory for exception stacks"))
        .collect();
    let exception_tops: Vec<usize> = exception_stacks.iter().map(KernelStack::top).collect();
    *AP_IDLE.lock() = Some(idle);

    let up = {
        // Interrupts off: the MD start sequence's delays must not be
        // stretched by preemption.
        let _g = intr_guard();
        // SAFETY: `cpu` is a detected AP, started only once (checked
        // above), and starts are serialized by STARTED plus the single boot
        // thread. The stacks are fresh, mapped, 16-byte aligned, and are
        // leaked below, so they stay valid forever and are used by nothing
        // else.
        unsafe { Md::cpu_start_ap(cpu, boot.top(), &exception_tops) }
    };
    // Whether or not it answered, the AP may run on these stacks one day.
    core::mem::forget(boot);
    core::mem::forget(exception_stacks);
    if !up {
        AP_IDLE.lock().take();
        return false;
    }
    // Wait until it is in the scheduler.
    while !ONLINE[cpu as usize].load(Ordering::Acquire) {
        crate::cpu::cpu_spinwait();
    }
    true
}

/// The AP start-up upcall (`mdi::ApStartupUpcall`, FreeBSD's
/// `init_secondary_tail`). Not for services.
#[doc(hidden)]
#[unsafe(no_mangle)]
pub fn __nova_ap_startup(cpuid: u32) -> ! {
    let idle = AP_IDLE
        .lock()
        .take()
        .expect("__nova_ap_startup: no idle thread");
    assert_eq!(
        curcpu().as_u32(),
        cpuid,
        "__nova_ap_startup: wrong per-CPU area"
    );
    crate::clock::cpu_initclocks_ap();
    mp_ncpus_inc();
    ONLINE[cpuid as usize].store(true, Ordering::Release);
    task::ap_enter(idle)
}

const _: mdi::ApStartupUpcall = __nova_ap_startup;

/// Sends `ipi` to `cpu` (`ipi_cpu`).
pub fn ipi_cpu(cpu: CpuId, ipi: Ipi) {
    Md::ipi_send(cpu.as_u32(), ipi);
}

/// Stops every other running CPU (FreeBSD's `stop_cpus_hard` on panic).
/// CPUs spinning with interrupts off do not stop.
///
/// TODO(smp): stop them with an NMI, as FreeBSD's `IPI_STOP_HARD` does.
pub fn stop_cpus() {
    let me = curcpu();
    for cpu in online_cpus().filter(|&c| c != me) {
        ipi_cpu(cpu, Ipi::Stop);
    }
}
