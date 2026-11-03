# Nova architecture

This document explains how the Nova kernel is put together and why. The
precise rules that keep it that way are in [LAYERING.md](LAYERING.md).

## Goals

1. **Run the FreeBSD 15.1 userland unmodified.** Compatibility is at the
   *syscall ABI* (syscall numbers, struct layouts, errno, `sysctl`,
   `ioctl`, signals, auxv), not at FreeBSD's internal kernel interfaces:
   Nova does not load FreeBSD `.ko` modules.
2. **Be boot-compatible with FreeBSD's `loader(8)`**, so Renux can swap
   kernels without touching anything else.
3. **Be a framekernel:** memory safety by construction, with all `unsafe`
   code confined to a small trusted framework.
4. **Be portable and SMP from the start**, with a strict separation
   between machine-dependent (MD) and machine-independent (MI) code.
5. **Be familiar:** FreeBSD/NetBSD directory and file names.

## The `sys/` tree

```
sys/
├── libkern/          base     trusted   MI building blocks: raw spin lock, ...
├── sys/              base               MI definitions shared by all layers (the sys/sys headers)
├── mdi/              base               the Machine-Dependent Interface: traits only
├── arch/
│   ├── x86/          md       trusted   shared x86 code: cpufunc, MSRs, early UART
│   └── amd64/        md       trusted   the amd64 port: locore, machdep, pcpu, ...
├── framework/        framework trusted  framekernel core: CPUs, per-CPU data, locks, ...
├── vm/               service            virtual memory policy
├── kern/             service            the kernel core: start-up, console, printf, panic, kenv, ...
├── compat/
│   └── freebsd/      service            the FreeBSD 15.1 ABI personality
└── conf/             conf               the kernel binary (like GENERIC) + ldscript.amd64
```

Planned, with the same rules: `fs/` (devfs, tmpfs), `ufs/`, `dev/`
(`dev/ic`, `dev/pci`, `dev/virtio`, ...), `net/`, `netinet/`.

Every directory is one Cargo crate. Sources sit directly in the directory
(no `src/`) and keep FreeBSD's file names: `kern/init_main.rs`,
`kern/subr_prf.rs`, `arch/amd64/machdep.rs`, `arch/amd64/locore.rs`, ...
Crate roots are `lib.rs` (or `main.rs` for `conf`).

## Framekernel

The kernel is split in two kinds of code.

**The framework** (the *trusted computing base*): `sys/framework`,
`sys/arch/*` and `sys/libkern`. It may use `unsafe`, `asm!` (MD only) and
raw pointers. It owns the **mechanisms**:

| Mechanism | Status |
|---|---|
| CPU identity, `MAXCPU`, `mp_ncpus` | done (`framework::cpu`) |
| Interrupt control through guards | done (`framework::cpu::IntrGuard`) |
| Per-CPU variables | done (`framework::pcpu::CpuLocal`) |
| Interrupt-safe spin mutexes | done (`framework::sync::MtxSpin`) |
| Early console | done (`framework::cons`) |
| Physical pages: `vm_phys` buddy allocator, owned `Frame`/`Segment` | done (`framework::mm`) |
| Kernel heap: Rust's `GlobalAlloc` (`Box`, `Vec`, ...) | done (`framework::mm::kmem`) |
| Clock interrupt, interrupt upcall | done (`framework::clock`, `framework::intr`) |
| Traps (MD: GDT/TSS/IDT, `trap_fatal`) and the direct map | done (`arch/amd64`) |
| Address spaces over the MD `pmap` | milestone 3–4 |
| Kernel threads: context switch, stacks with guard pages, preemption | done (`framework::task`, `framework::mm::kstack`) |
| Sleep mutexes, wait queues, `OnceLock` | done (`framework::sync`) |
| Pluggable scheduler (`Scheduler` trait, injected by a service) | done (`framework::task`) |
| SMP: CPU discovery (ACPI MADT), AP start-up, IPIs, `stop_cpus` | done (`framework::smp`, `arch/amd64/mp_machdep.rs`) |
| User memory access (`UserPtr`, `copyin`/`copyout`) | milestone 4 |
| Device access: `bus_space`, `bus_dma` (NetBSD style) | milestone 7 |

**The services**: everything else. They implement the **policies**
(scheduling, VM objects and faults, VFS, file systems, drivers, the network
stack, ABI personalities) in safe Rust, with `#![forbid(unsafe_code)]`.

The invariant: **every public API of the framework is sound.** No safe
code in a service, however buggy, can cause undefined behaviour. A
service bug can make the kernel misbehave or panic, but it cannot corrupt
memory. So reviews of the framework must be strict, and the framework
must stay small: when something can be written in safe Rust on top of the
framework, it belongs in a service.

This is the same split as BSD's `pmap`/`vm` and `bus_space`/drivers, made
strict and checked by the compiler. The model comes from the Asterinas
kernel (OSTD).

## MD and MI

```
        services (MI, safe)      kern  vm  compat/freebsd  fs  dev ...
                                   │
                                   ▼   safe, SMP-aware APIs
        framework (MI, trusted)  framework ──── selects the arch for the target,
                                   │            checks it implements all of mdi
                                   ▼   mdi traits (static dispatch)
        md (trusted)             arch/amd64 ── arch/x86
                                   │
        base (everyone)          libkern   sys   mdi
```

* **`mdi`** is the contract: traits (`Cpu`, `EarlyCons`, `Machine`) and
  plain data (`BootInfo`). It has no code. Reading it shows the whole MD
  surface of the kernel at once.
* **`sys/arch/<arch>`** implements every `mdi` trait on a zero-sized
  `Machine` type. It never depends on the framework or on services.
* **`framework`** is the only crate that names an architecture. It does
  so in exactly one place (`cfg(target_arch)` → `use amd64 as md`), checks
  at compile time that `md::Machine: mdi::Machine`, and wraps the traits
  in safe, MI functions. Dispatch is static: no `dyn`, and no generics
  leaking into services.
* **Services never see `mdi` or `arch/`.** They call
  `framework::cpu::curcpu()`, `framework::sync::MtxSpin`, ...

### Upcalls (MD → MI)

Some calls must go the other way: when the MD bootstrap is done it must
start the MI kernel, and later traps, interrupts and syscalls must reach MI
handlers. MD code cannot import MI crates, so upcalls go through symbols
or hooks declared in `mdi` and bound by the kernel binary:

* `mdi::MiStartup` (`fn(&BootInfo) -> !`) is bound with
  `framework::kernel_entry!(kern::init_main::mi_startup)` in `conf/main.rs`.
  The macro type-checks the function, so the `extern` declaration in
  `arch/amd64/machdep.rs` can rely on the signature.
* Traps, interrupts, syscalls and AP start-up will use a `KernelHooks`
  trait registered once by `mi_startup`.

### Porting to a new architecture

1. Write `sys/arch/<arch>` implementing all of `mdi`. The compiler lists
   anything missing.
2. Add one dependency line to `framework/Cargo.toml` and one `use` line to
   `framework/lib.rs`.
3. Add `sys/conf/ldscript.<arch>` and its case in `conf/build.rs`.

No service changes. If a service needs changes, the MD/MI split leaked
and the fix belongs in `mdi`/`framework`.

## SMP

Nova runs on one CPU today, but every API is designed for many:

* **CPU identity** comes from the MD per-CPU area. On amd64,
  `MSR_GSBASE` points at the CPU's `Pcpu`, so `curcpu()` is a single
  `%gs`-relative load (`arch/amd64/pcpu.rs`). Each CPU sets it up first
  thing (`pcpu_init` in `hammer_time`, later in `init_secondary`).
* **Per-CPU variables** (`CpuLocal<T>`): reaching your own slot needs a
  `PinnedToCpu` guard, which proves that interrupts are off, so nothing
  can preempt you, migrate you, or touch the same slot while you hold the
  reference. Other CPUs' slots are reachable only for `Sync` data. The
  compiler enforces both.
* **Interrupt control is guard-only.** Services get `IntrGuard`, never a
  raw "enable interrupts" call, so a guard cannot be invalidated behind
  its back.
* **Locks:** `MtxSpin` disables interrupts and spins; its guard also
  proves `PinnedToCpu`. Sleep locks come with the scheduler.
* **No `static mut`**, anywhere (rule R6). Shared state lives behind a
  lock, in an atomic, or in a `CpuLocal`.
* **Panic** is SMP-aware: one CPU wins the panic and stops the others
  with `IPI_STOP` (`stop_cpus_hard`).

### Starting the other CPUs

```
 hammer_time (BSP)          sys::acpi: RSDP (kenv acpi.rsdp, or BIOS scan)
   cpu_mp_setup ──────────►   -> XSDT -> MADT -> enabled local (x2)APICs
 mi_startup                 cpu ids: BSP = 0, then MADT order (as FreeBSD)
   release_aps
     framework::smp::start_ap(n)   idle thread, boot stack, 3 IST stacks
       amd64 cpu_start_ap          copy mpboot to 0x8000, INIT-SIPI-SIPI
         AP: mpboot                real -> protected -> long mode
         AP: init_secondary        kernel CR3, %gs, GDT/TSS/IST, x2APIC
         AP: __nova_ap_startup     clock, online, ap_enter -> idle thread
```

The APs share the run queue; an AP's idle thread sleeps in `hlt`, and
`sched_add`-style wakeups send it `IPI_AST`. The local APIC timer is
calibrated once, on the boot CPU, and reused by the APs. Only the boot
CPU's clock advances `ticks`.

Kernel stack addresses are never reused, so other CPUs' stale TLB entries
for a freed stack are harmless without TLB shootdown IPIs; see
`framework/mm/kstack.rs`.

Planned SMP pieces: TLB shootdown IPIs (to recycle kernel addresses and
for user address spaces), stopping CPUs with an NMI, per-CPU run queues
(`sched_ule`), per-CPU caches in `uma`, and xAPIC for CPUs without
x2APIC.

## The FreeBSD personality

FreeBSD binaries are Nova's native userland, but the ABI is still kept in
one place, `sys/compat/freebsd`, the way FreeBSD keeps its Linux ABI in
`sys/compat/linux`:

* the syscall table, **generated** from FreeBSD's
  `sys/kern/syscalls.master` by `tools/makesyscalls`;
* the `sysentvec`: syscall dispatch, errno translation, signal delivery,
  initial stack and auxv;
* `#[repr(C)]` structs with `const` layout assertions, whose expected
  values come from a dump made on a real FreeBSD 15.1;
* the MD part (`arch/amd64.rs`): the syscall register convention (errors
  are returned in `%rax` with the carry flag set, unlike Linux), the signal
  frame.

The MI kernel has its own internal interfaces and errno. Only the
personality translates. That keeps the kernel free to evolve, and leaves
room for other personalities next to FreeBSD's (a native Nova ABI, Linux).
Only `conf` links personalities in (rule R9).

## Memory

```
 physical memory (BootInfo::physavail)
        │
        ▼
 framework::mm::vm_phys   buddy allocator (algorithm: libkern::buddy,
        │                 links stored in the free pages via the DMAP)
        ├──► Frame / Segment   owned pages: freed on drop, zeroed on alloc,
        │                      contents copied in and out, never borrowed
        └──► mm::kmem          #[global_allocator]: 16 B–2 KiB slab classes,
                               larger requests as buddy blocks
```

The direct map (`DMAP_MIN_ADDRESS = 0xfffff80000000000`, FreeBSD's
address) is built by the MD `pmap_bootstrap` and promised to the
framework by the `unsafe trait mdi::DirectMap`. The framework's memory
code is sound only because of that contract, which is why it is an
`unsafe trait`.

Frame contents are reached by copying (`read_bytes`/`write_bytes`), never
through `&[u8]`, so that once frames can be mapped into user space, the
kernel never holds Rust references into memory userland can change.

## Traps and interrupts

```
 CPU ─► IDT gate ─► Xpage, Xtimerint, ... (arch/amd64/exception.rs)
                       push error code (dummy if none) + FreeBSD T_* code
                    ─► alltraps: save registers, call trap(&mut TrapFrame)
                    ─► trap (arch/amd64/trap.rs)
                         exception ─► trap_fatal: FreeBSD's report, panic
                         interrupt ─► EOI, upcall __nova_intr(Interrupt)
                                        ─► framework::intr ─► clock_intr
                                             ─► kern_clock::hardclock
```

The trap numbers and the fatal trap report are FreeBSD's (`Fatal trap 12:
page fault while in kernel mode`, `fault virtual address = ...`), so
panics read the same. The GDT layout is FreeBSD's slot for slot: it is
part of the userland ABI (selectors appear in signal contexts).

## Threads and scheduling

```
 framework::task (mechanism, trusted)       kern::sched_4bsd (policy, safe)
 ─────────────────────────────────────      ──────────────────────────────
 Task: stack, saved context, state          impl Scheduler {
 spawn / yield_now / exit / current             enqueue(task)   (sched_add)
 reschedule: pick, switch, finish_switch        pick_next()     (sched_choose)
 preempt_point at interrupt exit            }
 WaitQueue, Mutex                           sched_clock: slice of hz/10 ticks,
         ▲                                    then request_preempt()
         └──────── inject_scheduler(&SCHED) ──┘
```

As in Asterinas, the framework does the switching and the service only
decides who runs. FreeBSD's names are kept on both sides (`mi_switch`,
`sched_add`, `sched_choose`, `sched_clock`, `kthread_add`, `pause`).

The framework's rules, checked at run time where the compiler cannot:

* **Every switch happens with exactly one interrupt guard held.** A thread
  cannot switch away holding a spin lock (panic: "mi_switch: switch with
  spin locks or critical sections held"), and a sleep mutex cannot be
  taken holding one (panic: "acquiring blockable sleep lock with spinlock
  or critical section held", as WITNESS says in FreeBSD).
* **A context is never loaded before it is fully saved** (`on_cpu`), so a
  thread woken on one CPU cannot start on another before the first has
  switched away from it (SMP).
* **A thread's stack is freed only when its last reference goes away**,
  which is never before it has been switched away from for the last time.
* **Kernel stacks have an unmapped guard page**: an overflow is a double
  fault on its own IST stack, reported, not silent corruption.
* **Preemption** happens at interrupt exit (`__nova_intr_exit`, FreeBSD's
  `doreti` → `ast`), only when the interrupted code held no guard.

## Console messages

The kernel prints what FreeBSD prints, in the same format, wherever Nova
does the same thing: the boot report (`CPU:`, `Origin=`, `Features=`,
`Hypervisor:`, `real memory`, `avail memory`, `Event timer`,
`Timecounters tick every`), fatal traps (`Fatal trap 12: page fault while
in kernel mode`, `Fatal double fault`), panics (`panic:`, `cpuid =`,
`time =`, `KDB: stack backtrace:` with `#n 0x... at function+0x...`,
`Uptime:`, `Automatic reboot in 15 seconds - press a key on the console
to abort`) and halts (`The operating system has halted.`, `Please press
any key to reboot.`). The texts are copied from the FreeBSD 15.1 sources,
not from memory; see the "Messages" rule in [STYLE.md](STYLE.md).

Function names in backtraces come from the symbol table the loader
preloads (`MODINFOMD_SSYM`/`ESYM`, read by `sys::elf::KSymTab`), and Rust
names are demangled by `libkern::rust_demangle`.

## Boot

See [BOOT.md](BOOT.md). In short: FreeBSD's `loader(8)` loads the kernel
ELF at physical 2 MiB and jumps to `btext` in long mode. `btext`
(`arch/amd64/locore.rs`) switches to the boot stack and calls `hammer_time`
(`arch/amd64/machdep.rs`), which sets up the per-CPU area, the early
console, the GDT/TSS/IDT, the kernel's page tables and direct map, and
the local APIC, then turns the loader's metadata into an MI `BootInfo`.
The framework initializes the page allocator from it, and `mi_startup`
(`kern/init_main.rs`) runs.
