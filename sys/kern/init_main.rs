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
//! `boot -v` here too. The root file system is mounted from the memory
//! disk the loader preloaded, if any, and `init` started from it (or from
//! a file the loader preloaded).

use libkern::conv::u64_from_usize;
use core::sync::atomic::{AtomicBool, Ordering};

use framework::sync::OnceLock;
use framework::{BootInfo, PAGE_SIZE};
use sys::copyright::{COPYRIGHT, OSTYPE};
use sys::reboot::{RB_SINGLE, RB_VERBOSE};

use crate::kern_environment::StaticEnv;
use crate::subr_prf::Cons;
use crate::vers::{COMPILER, KERN_IDENT, OSRELEASE, VCS};
use crate::kern_proc::{Proc, proc_register};
use crate::{
    kern_clock, kern_exec, kern_kthread, kern_linker, kern_random, kern_shutdown, kern_synch, printf,
    sched_4bsd, subr_param, subr_selftest, subr_trap,
};

/// `bootverbose`: the loader was asked for a verbose boot (`boot -v`).
static BOOTVERBOSE: AtomicBool = AtomicBool::new(false);

/// True for a verbose boot: messages FreeBSD prints only then, or Nova's
/// own diagnostics, are printed.
pub fn bootverbose() -> bool {
    BOOTVERBOSE.load(Ordering::Relaxed)
}

/// The MI entry point (`mdi::MiStartup`). Called once by the MD bootstrap
/// on the boot CPU, with interrupts disabled; the framework (page
/// allocator, heap, thread 0) is already initialized, and `sys/conf` has
/// registered the ABIs, image activators, file systems and the
/// [`InitPolicy`].
pub fn mi_startup(boot: &BootInfo<'_>) -> ! {
    // The video console first, as FreeBSD's `cninit`: everything printed
    // from here on shows on the screen too.
    vt::vt_init(boot);
    let kenv = StaticEnv::new(boot.envp);
    if let Err(e) = crate::kern_environment::kenv_init(boot.envp) {
        panic!("cannot keep the kernel environment: {e:?}");
    }
    let bootverbose = boot.howto & RB_VERBOSE != 0;
    BOOTVERBOSE.store(bootverbose, Ordering::Relaxed);
    let param = subr_param::init_param1(&kenv);
    kern_shutdown::set_panic_reboot_wait_time(param.panic_reboot_wait_time);
    kern_linker::linker_init(boot);

    print_caddr_t();
    identify_cpu();
    cpu_startup(boot, &kenv, bootverbose);
    if bootverbose {
        linker_preload(boot);
    }
    kern_random::random_init(bootverbose);
    sched_4bsd::sched_init(param.hz);
    kern_clock::initclocks(param.hz);
    mp_start();
    kern_clock::inittimecounter();
    kern_clock::inittodr();
    framework::cpu::intr_enable();
    release_aps(bootverbose);

    subr_selftest::run(bootverbose, param.hz);
    subr_selftest::trap_selftest(&kenv);

    crate::vfs_mount::vfs_mountroot(boot, &kenv);
    if let Err(e) = kern_kthread::kthread_add("ttysig", || crate::tty::tty_signal_thread()) {
        panic!("cannot start the terminal signal thread: {e:?}");
    }
    if let Err(e) = kern_kthread::kthread_add("itimer", || crate::kern_resource::itimer_thread()) {
        panic!("cannot start the interval timer thread: {e:?}");
    }
    start_init(boot, &kenv, bootverbose);
    swapper(param.hz)
}

/// How the first user program is named and started. A personality
/// supplies it, so the kernel core does not hardcode another system's
/// userland conventions. `sys/conf` registers it before `mi_startup`.
pub trait InitPolicy: Sync {
    /// The paths tried for `init` when the environment has no
    /// `init_path` (FreeBSD's default is `/sbin/init:...`).
    fn init_path(&self) -> &'static str;

    /// The argument single-user mode adds after the program name
    /// (`-s` for FreeBSD). `None` for a personality that has no such
    /// mode or spells it differently.
    fn single_user_arg(&self) -> Option<&'static str> {
        Some("-s")
    }
}

static INIT_POLICY: OnceLock<&'static dyn InitPolicy> = OnceLock::new();

/// Registers the init policy. Called by the kernel binary before
/// `mi_startup`.
///
/// # Panics
///
/// If called more than once, or if no policy is registered when
/// `mi_startup` reaches `init`.
pub fn set_init_policy(policy: &'static dyn InitPolicy) {
    assert!(INIT_POLICY.set(policy).is_ok(), "init policy set twice");
}

fn init_policy() -> &'static dyn InitPolicy {
    *INIT_POLICY.get().expect("init_main: no init policy registered")
}

/// Starts the initial user process (`start_init`): execs the first file
/// of `init_path` found on the root file system, or else preloaded by the
/// loader (e.g. with `init_load="YES"`, `init_type="nova_exec"` and
/// `init_name="/sbin/init"` in loader.conf).
fn start_init(boot: &BootInfo<'_>, kenv: &StaticEnv<'_>, bootverbose: bool) {
    let policy = init_policy();
    let init_path = kenv.getenv("init_path").unwrap_or(policy.init_path());
    for path in init_path.split(':') {
        if bootverbose {
            printf!("start_init: trying {}\n", path);
        }
        // `init` gets its path and, in single-user mode, the policy's
        // extra argument (`-s` for FreeBSD).
        let mut args = [path, ""];
        let mut argc = 1;
        if boot.howto & RB_SINGLE != 0
            && let Some(arg) = policy.single_user_arg()
        {
            args[1] = arg;
            argc = 2;
        }
        let argv = args.get(..argc).unwrap_or(&args);
        let image = match crate::vfs_mount::rootvnode() {
            // On the root file system, through a process with no image yet
            // only for the lookup: the root is its directory.
            Some(root) => match crate::vfs_lookup::namei(root, root, path.as_bytes(), true) {
                Ok(vp) => kern_exec::kern_execve(&vp, path, argv, &[]),
                Err(e) => {
                    if bootverbose {
                        printf!("start_init: {}: {:?}\n", path, e);
                    }
                    continue;
                }
            },
            None => match kern_linker::preload_file(boot, path) {
                Some(file) => kern_exec::kern_execve(&kern_exec::MemFile(file), path, argv, &[]),
                None => continue, // ENOENT
            },
        };
        match image {
            Ok(image) => {
                let p = Proc::new(path, image.vm, image.abi).and_then(proc_register);
                let p = match p {
                    Ok(p) => p,
                    Err(e) => panic!("start_init: cannot create init: {e:?}"),
                };
                let regs = image.regs;
                let _ = p.set_args(argv);
                let me = p.clone();
                match kern_kthread::kthread_add("init", move || {
                    subr_trap::userret_loop(&p, regs);
                }) {
                    Ok(t) => me.tid.store(t.tid(), core::sync::atomic::Ordering::Relaxed),
                    Err(e) => panic!("start_init: cannot create init: {e:?}"),
                }
                return;
            }
            // The kernel has no error numbers; the ABI that would have
            // given one is the one that failed.
            Err(e) => printf!("exec {}: error {:?}\n", path, e),
        }
    }
    printf!("init: not found in path {}\n", init_path);
    panic!("no init");
}

/// What the boot thread becomes once the system is up (`swapper`).
fn swapper(hz: u32) -> ! {
    loop {
        kern_synch::pause("swapin", hz);
    }
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

/// Memory sizes, as FreeBSD's `cpu_startup` reports them.
fn cpu_startup(boot: &BootInfo<'_>, kenv: &StaticEnv<'_>, bootverbose: bool) {
    let free = framework::mm::vm_phys::free_count() * u64_from_usize(PAGE_SIZE);
    // SMBIOS knows the installed RAM; the firmware map only what is usable.
    let maxmem = boot.physmem.last().map_or(0, |r| r.end);
    let realmem = kenv
        .getenv("smbios.memory.enabled")
        .and_then(|v| v.parse::<u64>().ok())
        .map(|kib| kib << 10)
        .filter(|&m| m >= free)
        .unwrap_or(maxmem);
    crate::kern_mib::set_memory(realmem, free);
    printf!("real memory  = {} ({} MB)\n", realmem, realmem >> 20);
    if bootverbose {
        printf!("Physical memory chunk(s):\n");
        for r in boot.physavail {
            printf!(
                "0x{:016x} - 0x{:016x}, {} bytes ({} pages)\n",
                r.start,
                r.end - 1,
                r.len(),
                r.len() / u64_from_usize(PAGE_SIZE)
            );
        }
    }
    printf!("avail memory = {} ({} MB)\n", free, free / 1_048_576);
}

/// Reports the CPUs the firmware lists (the machine's probe line, then
/// `mp_start` and `cpu_mp_announce`). The APs are started later, by
/// `release_aps`.
fn mp_start() {
    let _ = framework::smp::cpu_mp_print_probe(&mut Cons);
    let ncpus = framework::smp::mp_ncpus_detected();
    if ncpus < 2 {
        return;
    }
    printf!("Nova/SMP: Multiprocessor System Detected: {} CPUs\n", ncpus);
    let (packages, cores, threads) = framework::smp::cpu_topology();
    // One line, built in place: printf! takes the console lock per call,
    // so the pieces are formatted together.
    printf!(
        "Nova/SMP: {} package(s){}{}\n",
        packages,
        Optional(cores > 0, format_args!(" x {cores} core(s)")),
        Optional(threads > 1, format_args!(" x {threads} hardware threads"))
    );
}

/// Starts the application processors (`release_aps`, with the messages
/// of `init_secondary_tail`).
/// Formats its arguments only if the flag is set.
struct Optional<'a>(bool, core::fmt::Arguments<'a>);

impl core::fmt::Display for Optional<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        if self.0 { f.write_fmt(self.1) } else { Ok(()) }
    }
}

fn release_aps(bootverbose: bool) {
    let ncpus = framework::smp::mp_ncpus_detected();
    for cpu in 1..ncpus {
        if !framework::smp::start_ap(cpu) {
            panic!(
                "AP #{} (PHY# {}) failed!",
                cpu,
                framework::smp::cpu_hwid(cpu)
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
