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

//! `mdi`: the **Machine-Dependent Interface**.
//!
//! This crate is the contract between the machine-independent kernel and
//! each architecture, and nothing else: traits and plain data types, no
//! code. It plays the role of the MD function prototypes that FreeBSD and
//! NetBSD scatter over `machine/cpufunc.h`, `vm/pmap.h`, `sys/cons.h`...,
//! gathered in one place so the whole MD surface can be read at once.
//!
//! Two directions of calls cross this boundary:
//!
//! * **Downcalls (MI → MD)**: the `framework` crate calls the traits
//!   below and wraps them in safe, SMP-aware APIs for the services. Every
//!   architecture crate implements all of them on a zero-sized `Machine`
//!   type; `framework` checks that at compile time, so a missing function
//!   is a build error, not a link error. Dispatch is static: no `dyn`, no
//!   generics in MI code.
//! * **Upcalls (MD → MI)**: when the MD bootstrap is done it calls
//!   [`MiStartup`], bound by the kernel binary with
//!   `framework::kernel_entry!`. Interrupts go to [`IntrUpcall`] and
//!   [`IntrExitUpcall`], and each AP to [`ApStartupUpcall`], all defined
//!   by the framework. Both are link-time symbols whose types are checked
//!   against the aliases here, so MD code never imports MI code.
//!
//! Only the framework calls these traits; services never see them. Where
//! a mistake could break memory safety, the contract is written down: an
//! `unsafe trait` when the MD implementation must keep a promise
//! (`DirectMap`, `KernelMap`), an `unsafe fn` when the caller must
//! (`Context::cpu_switch`).
//!
//! Adding a method here forces every architecture to implement it. Keep
//! the interface small and in MI terms; see docs/LAYERING.md.

#![no_std]
// `deny`, not `forbid`: declaring an `unsafe trait` or `unsafe fn` (a
// contract one side must uphold) counts as unsafe code, and is the one
// thing allowed here, item by item.
#![deny(unsafe_code)]

pub mod bootinfo;

use core::fmt;

pub use bootinfo::{BootInfo, FbInfo, Firmware};
pub use sys::physmem::PhysRange;

/// CPU control.
pub trait Cpu {
    /// Saved interrupt state, returned by [`Cpu::intr_disable`]. Opaque
    /// to MI code, so it can only be handed back to [`Cpu::intr_restore`].
    type IntrState: Copy;

    /// Disables interrupts on this CPU and returns the previous state.
    fn intr_disable() -> Self::IntrState;

    /// Restores the interrupt state saved by [`Cpu::intr_disable`].
    fn intr_restore(state: Self::IntrState);

    /// Enables interrupts on this CPU. The framework only calls it when no
    /// interrupt guard is held.
    fn intr_enable();

    /// Enables interrupts and sleeps until the next one (`cpu_idle`);
    /// returns with interrupts enabled. Same precondition as `intr_enable`.
    fn cpu_idle();

    /// Hint that the CPU is in a spin-wait loop.
    fn cpu_spinwait();

    /// Id of the executing CPU, `0..MAXCPU`; the boot CPU is 0. Must be
    /// valid from the first instruction of `mi_startup` on every CPU.
    fn curcpu() -> u32;

    /// Disables interrupts and stops this CPU for good.
    fn cpu_halt() -> !;

    /// Deliberately takes a fatal kernel page fault (a read of an
    /// unmapped address), to exercise the trap path. Never returns.
    fn cpu_selftest_fault() -> !;

    /// Busy-waits for at least `us` microseconds (`DELAY`). Works with
    /// interrupts disabled.
    fn delay(us: u32);

    /// Resets the machine (`cpu_reset`).
    fn cpu_reset() -> !;

    /// Walks the caller's stack, innermost frame first, calling `f` with
    /// each frame until it returns false or the chain ends, crossing trap
    /// frames into the interrupted code (`db_backtrace`). Must never
    /// fault, whatever the stack contains.
    fn stack_trace(f: &mut dyn FnMut(StackFrame) -> bool);

    /// Writes the boot CPU's identification, in FreeBSD's format
    /// (`printcpuinfo`: the `CPU:`, `Origin=` and `Features=` lines, and
    /// `Hypervisor:` when virtualized).
    fn cpu_print_info(w: &mut dyn fmt::Write) -> fmt::Result;
}

/// One step of a stack trace.
#[derive(Clone, Copy, Debug)]
pub enum StackFrame {
    /// A return address: the call is just before it.
    Return(usize),
    /// A trap or interrupt frame: the code at `pc` was interrupted, with
    /// stack pointer `sp` and frame pointer `fp`.
    Trap {
        /// An interrupt rather than an exception.
        interrupt: bool,
        /// The MD trap code (exceptions only), printed as is.
        trapno: u64,
        /// Interrupted instruction.
        pc: usize,
        /// Interrupted stack pointer.
        sp: usize,
        /// Interrupted frame pointer.
        fp: usize,
    },
    /// The interrupted instruction itself (follows a `Trap`).
    Pc(usize),
}

/// Kernel thread contexts (FreeBSD's `struct pcb`, `cpu_switch`).
#[allow(unsafe_code)]
pub trait Context {
    /// The saved state of a thread that is not running.
    type Pcb: Send;

    /// A `Pcb` that has never run: only valid as the `old` side of the
    /// first `cpu_switch` (the boot thread).
    const PCB_INIT: Self::Pcb;

    /// Prepares `pcb` so that switching to it calls `entry` on the stack
    /// ending at `stack_top`, with interrupts disabled.
    ///
    /// # Safety
    ///
    /// `stack_top` must be the 16-byte aligned top of a mapped, writable
    /// stack that stays valid for as long as the thread can run; `entry`
    /// must never return.
    unsafe fn cpu_thread_init(pcb: &mut Self::Pcb, stack_top: usize, entry: extern "C" fn() -> !);

    /// Saves the running thread's state in `old` and resumes `new`
    /// (`cpu_switch`). Returns when some CPU switches back to `old`.
    ///
    /// # Safety
    ///
    /// Interrupts must be disabled. `old` must stay valid until it is
    /// resumed. `new` must come from `cpu_thread_init` or a previous
    /// `cpu_switch`, and must not be running or being saved on any CPU.
    unsafe fn cpu_switch(old: *mut Self::Pcb, new: *const Self::Pcb);
}

/// Mappings of single pages in the kernel's own address range (FreeBSD's
/// `pmap_kenter`, `pmap_kremove`, `pmap_kextract`).
///
/// # Safety
///
/// Implementations must map exactly what is asked, visible to every CPU,
/// only inside `[KVA_MIN, KVA_MAX)`, a range the MD layer uses for
/// nothing else, and must never fault in `pmap_kextract`.
#[allow(unsafe_code)]
pub unsafe trait KernelMap {
    /// Start of the range the framework manages.
    const KVA_MIN: usize;
    /// End of the range the framework manages.
    const KVA_MAX: usize;

    /// Maps the page at `va` to physical page `pa`, read-write, kernel
    /// only. `alloc` must return zeroed pages for any page table needed.
    /// Returns false if `alloc` failed.
    ///
    /// # Safety
    ///
    /// `va` must be page aligned, in `[KVA_MIN, KVA_MAX)` and unmapped;
    /// `pa` must be a page the caller owns and lets no one else use while
    /// mapped. Calls must be serialized.
    unsafe fn pmap_kenter(va: usize, pa: u64, alloc: &mut dyn FnMut() -> Option<u64>) -> bool;

    /// Unmaps the page at `va` and flushes it from the TLB.
    ///
    /// # Safety
    ///
    /// `va` must have been mapped with `pmap_kenter`, and nothing may use
    /// the mapping anymore. Calls must be serialized.
    unsafe fn pmap_kremove(va: usize);

    /// The physical address `va` maps to in the kernel, if any.
    fn pmap_kextract(va: usize) -> Option<u64>;
}

/// The direct map: all of physical memory, mapped at a fixed offset in
/// kernel virtual space (FreeBSD's DMAP, `PHYS_TO_DMAP`).
///
/// # Safety
///
/// The framework builds safe abstractions (page frames, the kernel heap)
/// on these promises, so an implementation must guarantee that, from the
/// moment `mi_startup` runs and for the life of the kernel:
///
/// * every physical address below [`DirectMap::dmap_limit`] is mapped
///   read-write, write-back cacheable, at `phys_to_dmap(pa)`, on every CPU;
/// * the mapping never changes and is never aliased by another kernel
///   mapping that the MD layer hands out;
/// * `dmap_to_phys(phys_to_dmap(pa)) == pa`.
#[allow(unsafe_code)]
pub unsafe trait DirectMap {
    /// Kernel virtual address of physical address `pa`.
    /// Panics if `pa >= dmap_limit()`.
    fn phys_to_dmap(pa: u64) -> usize;

    /// Physical address of a direct-map virtual address.
    /// Panics if `va` is not in the direct map.
    fn dmap_to_phys(va: usize) -> u64;

    /// End of the physical range covered by the direct map.
    fn dmap_limit() -> u64;
}

/// The per-CPU periodic clock (FreeBSD's event timer).
pub trait Clock {
    /// Event timer name, as FreeBSD reports it (`Event timer "LAPIC"`).
    const ET_NAME: &'static str;
    /// Event timer quality, as FreeBSD reports it.
    const ET_QUALITY: i32;

    /// Starts the periodic clock interrupt on this CPU at `hz` ticks per
    /// second. Each tick is delivered as [`Interrupt::Clock`].
    fn cpu_initclocks(hz: u32);

    /// The battery-backed real-time clock, in seconds since the Epoch
    /// (UTC), if the machine has one that reads sanely (FreeBSD's
    /// `atrtc` on the PC).
    fn rtc_gettime() -> Option<u64>;
}

/// An interrupt delivered by the MD layer to the framework.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Interrupt {
    /// A tick of the per-CPU clock started by [`Clock::cpu_initclocks`].
    Clock,
    /// An inter-processor interrupt sent with [`Smp::ipi_send`].
    Ipi(Ipi),
}

/// Inter-processor interrupts (FreeBSD's `IPI_*`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ipi {
    /// Reschedule soon: something became runnable (`IPI_AST`).
    Ast,
    /// Stop for good: another CPU panicked (`IPI_STOP`).
    Stop,
}

/// Multiprocessor support (FreeBSD's `cpu_mp_probe`, `cpu_mp_start`,
/// `ipi_cpu`). CPU ids are dense, `0..cpu_mp_probe()`, the boot CPU being 0.
#[allow(unsafe_code)]
pub trait Smp {
    /// Number of CPUs the firmware lists, the boot CPU included, capped at
    /// `MAXCPU` (`cpu_mp_probe`). 1 if there is no table.
    fn cpu_mp_probe() -> u32;

    /// CPU topology: packages, cores per package, hardware threads per
    /// core, as FreeBSD's `cpu_mp_announce` reports them.
    fn cpu_topology() -> (u32, u32, u32);

    /// How many extra stacks each AP needs for exceptions that cannot run
    /// on the interrupted stack (double fault, NMI...). The framework
    /// allocates them and passes them to [`Smp::cpu_start_ap`].
    const AP_EXCEPTION_STACKS: usize;

    /// The hardware id of CPU `cpuid` (FreeBSD's `cpu_hwid`): what the
    /// interrupt controller calls it.
    fn cpu_hwid(cpuid: u32) -> u32;

    /// Writes what FreeBSD prints when it probes the CPUs from firmware
    /// tables (on x86, `ACPI APIC Table: <...>`), if anything.
    fn cpu_mp_print_probe(w: &mut dyn fmt::Write) -> fmt::Result;

    /// Starts CPU `cpuid` (`start_ap`). It sets itself up (per-CPU area,
    /// descriptor tables, interrupt controller) on the stack ending at
    /// `stack_top`, with `exception_stack_tops` (exactly
    /// [`Smp::AP_EXCEPTION_STACKS`] of them) for the exceptions that need
    /// their own stack, then calls [`ApStartupUpcall`]. Returns false if
    /// it did not come up.
    ///
    /// # Safety
    ///
    /// `cpuid` must be in `1..cpu_mp_probe()` and not started yet; the
    /// stacks must be 16-byte aligned tops of mapped, writable stacks that
    /// stay valid forever and are used by nothing else. Calls must be
    /// serialized.
    unsafe fn cpu_start_ap(cpuid: u32, stack_top: usize, exception_stack_tops: &[usize]) -> bool;

    /// Sends `ipi` to CPU `cpuid` (`ipi_cpu`).
    fn ipi_send(cpuid: u32, ipi: Ipi);
}

/// Signature of the AP start-up upcall, `framework::smp::__nova_ap_startup`.
/// Each application processor calls it once set up, with interrupts
/// disabled, on the stack given to [`Smp::cpu_start_ap`].
pub type ApStartupUpcall = fn(cpuid: u32) -> !;

/// Signature of the interrupt upcall, `framework::intr::__nova_intr`. The
/// MD trap code calls it with interrupts disabled, after acknowledging the
/// interrupt controller.
pub type IntrUpcall = fn(Interrupt);

/// Signature of the interrupt exit upcall, `framework::intr::__nova_intr_exit`.
/// The MD trap code calls it after [`IntrUpcall`] returns, still with
/// interrupts disabled, on the interrupted thread's stack, right before
/// returning to it: this is where the framework may preempt that thread
/// (FreeBSD's `doreti` -> `ast`).
pub type IntrExitUpcall = fn();

/// Console available from the first instruction, before any driver is
/// attached (FreeBSD's early `cninit` path). Output only.
pub trait EarlyCons {
    /// Writes one byte, as is (no newline translation).
    fn putc(c: u8);

    /// A byte typed on the console, if any (`cncheckc`). Never blocks.
    fn checkc() -> Option<u8>;
}

/// Access rights of a user page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Prot {
    /// Readable. A page that is not is not mapped at all (machines may not
    /// support write- or execute-only pages; reads are then allowed).
    pub read: bool,
    /// Writable.
    pub write: bool,
    /// Executable.
    pub exec: bool,
}

/// Why user mode returned to the kernel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UserExit {
    /// The program asked for a system call. Which one, and its arguments,
    /// are in the registers, which only the program's ABI interprets.
    Syscall,
    /// An interrupt was taken (and handled); nothing to do but go back.
    Interrupt,
    /// The program faulted.
    Fault(UserFault),
}

/// A fault taken in user mode, in machine-independent terms.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UserFault {
    /// What happened.
    pub kind: FaultKind,
    /// The faulting address, for memory faults (else 0).
    pub addr: u64,
}

/// Kinds of user faults (what FreeBSD's `trap()` turns into signals; the
/// ABI decides which signal, if it has signals at all).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FaultKind {
    /// Access to an unmapped address.
    NotMapped {
        /// It was a write.
        write: bool,
    },
    /// Access that the mapping does not allow.
    Protection {
        /// It was a write.
        write: bool,
    },
    /// Privileged or invalid instruction.
    IllegalInstruction,
    /// Arithmetic error (division by zero, overflow trap...).
    Arithmetic,
    /// Breakpoint or single-step trap.
    Breakpoint,
    /// Anything else (the MD trap code).
    Other(u64),
}

/// User address spaces and user mode (FreeBSD's `pmap_pinit`,
/// `pmap_enter`, `pmap_activate`, and the return to user mode).
///
/// # Safety
///
/// Implementations must keep user mode from reaching kernel memory: a
/// user address space maps only what `pmap_enter_user` was given, below
/// `VM_MAXUSER_ADDRESS`, plus the kernel half without user access; and
/// `cpu_user_enter` must sanitize the registers (privilege bits, address
/// validity) so that no register value makes the CPU leave user mode
/// other than back through `cpu_user_enter`'s return.
#[allow(unsafe_code)]
pub unsafe trait UserMode {
    /// The user register file of this machine: what a program sees in its
    /// registers. Opaque to MI code; ABIs read and write it in their
    /// `arch/<arch>/` half.
    type UserRegs: Default + Send;

    /// First address above user space.
    const VM_MAXUSER_ADDRESS: u64;

    /// Creates an empty user address space and returns its root page
    /// table. `alloc` must return zeroed pages.
    fn pmap_pinit(alloc: &mut dyn FnMut() -> Option<u64>) -> Option<u64>;

    /// Maps user page `va` to physical page `pa` in the space `root`.
    /// Returns false if `alloc` failed.
    ///
    /// # Safety
    ///
    /// `root` must come from `pmap_pinit` and not be released; `va` must be
    /// page aligned, below `VM_MAXUSER_ADDRESS` and unmapped; `pa` must be
    /// a page owned by the caller for as long as it is mapped. Calls on one
    /// `root` must be serialized.
    unsafe fn pmap_enter_user(
        root: u64,
        va: u64,
        pa: u64,
        prot: Prot,
        alloc: &mut dyn FnMut() -> Option<u64>,
    ) -> bool;

    /// Unmaps user page `va` of the space `root` and returns the physical
    /// page it mapped, if any. The TLB of this CPU is updated if `root` is
    /// active here; other CPUs must reload `root` before running it again.
    ///
    /// # Safety
    ///
    /// As for `pmap_enter_user`: `root` live, calls serialized.
    unsafe fn pmap_remove_user(root: u64, va: u64) -> Option<u64>;

    /// Frees the page tables of the space `root` (not the pages they map),
    /// calling `free` with each.
    ///
    /// # Safety
    ///
    /// No CPU may be using `root`, and it must never be used again.
    unsafe fn pmap_release(root: u64, free: &mut dyn FnMut(u64));

    /// Makes `root` the current address space of this CPU (0: the kernel's
    /// only), flushing its non-global TLB entries.
    ///
    /// # Safety
    ///
    /// `root` must be 0 or a live space from `pmap_pinit`, and must stay
    /// live while it is current on any CPU.
    unsafe fn pmap_activate(root: u64);

    /// Runs the current thread in user mode, in the current address space,
    /// with `regs`, until it returns to the kernel; `regs` then holds the
    /// program's registers at that point. Called and returns with
    /// interrupts disabled; interrupts are enabled in user mode.
    ///
    /// # Safety
    ///
    /// The current address space must be the program's, and no interrupt
    /// guard may be held (interrupts taken in user mode are handled, and may
    /// switch threads, before this returns).
    unsafe fn cpu_user_enter(regs: &mut Self::UserRegs) -> UserExit;

    /// Bytes of the user floating-point state [`UserMode::fpu_save`]
    /// writes (the machine's format, e.g. amd64's `fxsave` area).
    const FPU_STATE_SIZE: usize;

    /// Copies the current thread's user floating-point state into `buf`
    /// (at least [`UserMode::FPU_STATE_SIZE`] bytes), for a signal frame.
    fn fpu_save(buf: &mut [u8]);

    /// Makes `state` (as [`UserMode::fpu_save`] wrote it, possibly changed
    /// by the program) the current thread's user floating-point state,
    /// with the bits a program may not set cleared; `None`: the initial
    /// state, as a new thread has.
    fn fpu_load(state: Option<&[u8]>);
}

/// Everything an architecture must provide.
pub trait Machine: Cpu + EarlyCons + DirectMap + KernelMap + Clock + Context + Smp + UserMode {
    /// Machine name, `hw.machine` (e.g. `"amd64"`).
    const MACHINE: &'static str;
    /// Machine architecture, `hw.machine_arch` (e.g. `"amd64"`).
    const MACHINE_ARCH: &'static str;
    /// Base page size in bytes.
    const PAGE_SIZE: usize;
    /// Maximum number of CPUs (`MAXCPU`).
    const MAXCPU: usize;
    /// Pages of each kernel thread stack (`KSTACK_PAGES`).
    const KSTACK_PAGES: usize;
    /// Block orders of the physical page allocator (`VM_NFREEORDER`): it
    /// hands out blocks of 1 to 2^(n-1) contiguous pages.
    const VM_NFREEORDER: usize;

    /// True when running under a hypervisor (FreeBSD's `vm_guest`).
    fn vm_guest() -> bool;

    /// Fills `out` from the machine's hardware random source, if it has
    /// one (FreeBSD's `random_ivy`/`rdseed` sources); false otherwise.
    fn cpu_random(out: &mut [u8]) -> bool;
}

/// Signature of the MI entry point, `kern::init_main::mi_startup`. The MD
/// bootstrap calls it exactly once, on the boot CPU, with interrupts
/// disabled, the early console ready, traps handled and the direct map
/// in place.
pub type MiStartup = fn(&BootInfo<'_>) -> !;
