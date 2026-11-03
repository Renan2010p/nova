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
| Physical frames (`vm_phys`), kernel heap (`GlobalAlloc`) | milestone 2 |
| Address spaces over the MD `pmap` | milestone 2 |
| Traps, interrupts, IPIs | milestone 2–3 |
| Threads: context switch, kernel stacks | milestone 3 |
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
* **Panic** is SMP-aware: one CPU wins the panic, the others stop. IPIs to
  stop them come with the APIC.

Planned SMP pieces: AP start-up (`arch/amd64/mp_machdep.rs`, INIT-SIPI-SIPI
from the local APIC, an `mpboot` trampoline), IPIs, TLB shootdown in
`pmap`, per-CPU run queues in the scheduler (`sched_ule`), per-CPU caches
in `uma`.

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

## Boot

See [BOOT.md](BOOT.md). In short: FreeBSD's `loader(8)` loads the kernel
ELF at physical 2 MiB and jumps to `btext` in long mode. `btext`
(`arch/amd64/locore.rs`) switches to the boot stack and calls `hammer_time`
(`arch/amd64/machdep.rs`). That sets up the per-CPU area and the early
console, turns the loader's metadata into an MI `BootInfo`, and calls
`mi_startup` (`kern/init_main.rs`).
