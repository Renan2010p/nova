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
//! devices, root mount, and finally `init(8)`. Nova will grow the same
//! ordered list. For now (milestone 1) it reports what the boot handed
//! over and halts.

use framework::BootInfo;
use sys::copyright::{COPYRIGHT, OSTYPE};
use sys::physmem;
use sys::reboot::RB_VERBOSE;

use crate::kern_environment::StaticEnv;
use crate::kern_shutdown::kern_halt;
use crate::printf;

const MIB: u64 = 1024 * 1024;

/// The MI entry point (`mdi::MiStartup`). Called once by the MD bootstrap
/// on the boot CPU, with interrupts disabled.
pub fn mi_startup(boot: &BootInfo<'_>) -> ! {
    printf!("{}", COPYRIGHT);
    printf!(
        "{} {} {}\n",
        OSTYPE,
        env!("CARGO_PKG_VERSION"),
        framework::MACHINE
    );
    let verbose = boot.howto & RB_VERBOSE != 0;

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

    let kenv = StaticEnv::new(boot.envp);
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

    printf!(
        "CPUs: {} running (boot CPU is cpu{}), up to {} supported\n",
        framework::cpu::mp_ncpus(),
        framework::cpu::curcpu().as_u32(),
        framework::MAXCPU
    );

    printf!("mi_startup: nothing more to do yet (milestone 1)\n");
    kern_halt()
}
