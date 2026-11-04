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
   `framework/arch/mod.rs`, the only file in the MI kernel that names a
   machine.
3. Add `sys/conf/ldscript.<arch>` and its case in `conf/build.rs`.
4. Give each ABI its MD half: `compat/<abi>/arch/<arch>/`.

No service changes. If a service needs changes, the MD/MI split leaked
and the fix belongs in `mdi`/`framework`.

This is not a promise, it is checked: `sys/arch/riscv64` is a
compile-only port (every operation panics) and `make check` builds the
whole kernel for it (rule R14). It was added with no change to any MI
file, after rule R13 had found and removed the last x86 terms from MI
code (`cpu_apic_id`, three "IST" stacks in the contract, the MADT parser
and its boot message, the kernel stack size and allocator orders).

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

## ABIs: the kernel knows none

The kernel does not know any userland ABI. It knows one interface,
`kern::kern_abi::SysEntVec` (FreeBSD's `struct sysentvec` as a trait), and
every ABI lives in `sys/compat/<abi>`, with its MD half in
`sys/compat/<abi>/arch/<arch>/`. Only `sys/conf` registers ABIs.

| The ABI (`sys/compat/<abi>`) | The kernel (`sys/kern`) |
|---|---|
| recognizes its images (`accepts`: brand, note, `EI_OSABI`) | loads `PT_LOAD` segments, maps the stack (`kern_execve`) |
| lays out argv, envp, auxv and the start registers (`exec_setregs`) | runs the program (`subr_trap::userret_loop`) |
| decodes a system call, encodes the result (`syscall`) | implements it (`kern_write`, `exit1`, ...) |
| says which signal a fault raises (`trapsignal`) | delivers it (for now: kills, with FreeBSD's log line) |
| numbers errors and signals (`signal_number`, its errno map) | names them only: `Errno`, `Signal` |

`checklayers` rule R15 keeps ABI names out of every other crate, and the
`Errno` and `Signal` enums are exhaustive, so a new kernel error does not
compile until every ABI has a number for it. A Linux, NetBSD or native
Nova ABI is "just" a new `sys/compat/<abi>`.

Still to generalize when a second ABI arrives: the image format (the
kernel loads ELF itself, as FreeBSD's `imgact_elf` serves several ELF
ABIs; a non-ELF ABI would bring its own image activator, FreeBSD's
`execsw`), and the set of signals and errors (grown as needed).

### The FreeBSD personality

FreeBSD binaries are Nova's native userland, through `sys/compat/freebsd`:

* `freebsd_syscalls.rs`: numbers and names, **generated** from FreeBSD's
  `sys/kern/syscalls.master` by `tools/makesyscalls` (`make sysent`); its
  output is identical to FreeBSD's own `syscalls.c` and `syscall.h`;
* `freebsd_sysent.rs`: the table: decodes arguments, calls the kernel. A
  number FreeBSD lacks is `nosys` (`SIGSYS`); one Nova lacks yet is
  `ENOSYS`, logged on a verbose boot;
* `freebsd_sysvec.rs`: brand matching, start-up, syscall entry;
* `freebsd_exec.rs`: the initial stack, laid out as FreeBSD's
  `exec_copyout_strings` and `freebsd_copyout_auxargs` do;
* `freebsd_errno.rs`, `freebsd_signal.rs`: numbers, and `trap()`'s
  fault-to-signal policy;
* `arch/amd64/`: `USRSTACK`, `exec_setregs` (`%rdi` = stack base, `%rsp`
  as after a call), arguments in `%rdi %rsi %rdx %r10 %r8 %r9`, errors in
  `%rax` with the carry flag set; `arch/riscv64/`: `t0`/`a0`-`a7`.

Planned: `#[repr(C)]` structs with `const` layout assertions checked
against a dump made on a real FreeBSD 15.1 (`tools/abidump`), sysctls,
ioctls, the signal frame.

## User mode

```
 kern::subr_trap::userret_loop(proc, regs)
   loop: framework::user::enter(&mut regs)          (no guard held)
           Md::cpu_user_enter ─► nova_user_enter: save kernel context,
                                 TSS.rsp0, swapgs, iretq to user mode
           ◄─ syscall      Xfast_syscall: swapgs, save user registers,
                           return to cpu_user_enter         ─► abi.syscall
           ◄─ interrupt    alltraps_user ─► user_trap: save registers,
                           trap() (may preempt), return     ─► loop
           ◄─ exception    alltraps_user ─► user_trap: FaultKind
                                                       ─► abi.trapsignal
```

Entering user mode is a function call that returns when the program comes
back, so the kernel side of a process is plain Rust code on its thread's
stack. `framework::user::VmSpace` owns every page it maps; the kernel
reads and writes user memory only by copying (`copyin`, `copyout`), never
through a user pointer. The scheduler activates a thread's address space
when it runs it; kernel threads run in whichever is active (the kernel
half is shared), and each CPU holds a reference to its active space, so a
space is freed only when no CPU can use it.

`init` is the first file of `init_path` (FreeBSD's default
`/sbin/init:/sbin/oinit:/sbin/init.bak:/rescue/init`) that the loader
preloaded, since there is no root file system yet. As in FreeBSD, its exit
is fatal: `init died (signal 0, exit 0)`, `panic: Going nowhere without my
init!`.

## Drivers

Drivers will be framekernel **services** in `sys/dev`, with
`#![forbid(unsafe_code)]` like the rest: they reach the hardware only
through framework abstractions (MMIO, I/O ports, DMA, interrupts), so a
buggy driver cannot corrupt kernel memory. Because a driver sees only that
interface, it can later be served by a userspace driver host as well, for
untrusted or third-party drivers, once Nova has fast IPC, an IOMMU and
interrupts delivered to user mode. The GUI is userland already, as on
FreeBSD (Xorg, Wayland); the kernel will provide the interfaces it needs
(DRM/KMS, evdev, devfs).

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
through `&[u8]`, so the kernel never holds Rust references into memory
userland can change (see "User mode").

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
