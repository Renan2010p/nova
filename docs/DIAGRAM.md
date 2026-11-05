# How Nova works — diagrams

A visual tour of the kernel, from the loader's jump to the userland
syscall loop. Each diagram is written in Mermaid, so it renders on a
forge (GitHub, GitLab, ...) and stays reviewable as text.

The diagrams describe the code in this tree; the
[ROADMAP](ROADMAP.md) has the milestone-by-milestone status.

---

## 1. The whole kernel on one page

```
        UEFI / BIOS / bhyve
                │  loads /boot/kernel/kernel
                ▼
        FreeBSD loader(8)
                │  long mode, paging on, modulep + kernend on the stack
                ▼
┌───────────────────────────────────────────────────────────────────────┐
│  Nova kernel image  (sys/conf — like FreeBSD's GENERIC)               │
│                                                                       │
│  services — 100% safe Rust, #![forbid(unsafe_code)]                   │
│  ┌──────────┬──────────┬──────────────┬──────────────┬────────────┐   │
│  │ kern     │ vm       │ exec/elf     │ compat/      │ dev/vt     │   │
│  │ init,    │ fault,   │ ELF image    │ freebsd      │ video      │   │
│  │ proc,    │ mmap     │ activator    │ (ABI)        │ console    │   │
│  │ exec,    │          │              │              │            │   │
│  │ VFS, tty │          │ fs/freebsd/  │              │            │   │
│  └────┬─────┴────┬─────┴──────┬───────┴──────┬───────┴─────┬──────┘   │
│       │          │            │              │             │          │
│       ▼          ▼            ▼              ▼             ▼          │
│  framework — trusted, may use unsafe (the only bridge to MD)          │
│  ┌─────────┬──────────────┬────────────────────┬──────────────────┐   │
│  │ cpu     │ task         │ mm                 │ sync / user /    │   │
│  │ pcpu    │ threads,     │ vm_phys, Frame,    │ cons / fb /      │   │
│  │ intr    │ scheduler    │ kmem, malloc,      │ clock / smp      │   │
│  │         │ mechanism    │ kstack             │                  │   │
│  └────┬────┴──────┬───────┴─────────┬──────────┴────────┬─────────┘   │
│       │           │                 │                   │             │
│       ▼           ▼                 ▼                   ▼             │
│  base: libkern (buddy, spinlock, chacha20, ...)                       │
│        sys     (errno, signal, elf, kenv, physmem, ...)               │
│        mdi     (traits only: the whole MD contract)                   │
│       │                                                               │
│       ▼  static dispatch through the mdi traits                       │
│  MD — trusted: arch/amd64 + arch/x86   (arch/riscv64 is check-only)   │
└───────────────────────────────────┬───────────────────────────────────┘
                                    ▼
                       CPU, APIC, COM1, framebuffer, RAM
```

The arrow between layers is a dependency arrow, never the reverse: MD
code cannot reach the framework or a service except through the upcalls
declared in `mdi` and bound by `sys/conf`.

---

## 2. Crate dependencies (what `checklayers` enforces)

Arrows point at the dependency.

```mermaid
flowchart BT
    conf["conf<br/>the kernel binary"]
    compat["compat_freebsd<br/>FreeBSD ABI"]
    exec_elf["exec_elf<br/>ELF image activator"]
    kpi["freebsd_kpi<br/>FreeBSD KPI (vendor/freebsd/kpi)"]
    ufs["ufs<br/>UFS2, read-only (sys/fs/freebsd/ufs)"]
    kern["kern<br/>MI kernel core"]
    vm["vm<br/>VM policy"]
    vt["dev/vt<br/>video console"]
    fw["framework<br/>trusted core"]
    amd64["arch/amd64"]
    x86["arch/x86"]
    riscv["arch/riscv64<br/>compile-only"]
    mdi["mdi<br/>traits only"]
    sys["sys<br/>shared definitions"]
    libkern["libkern<br/>base library"]

    conf --> compat
    conf --> exec_elf
    conf --> ufs
    conf --> kern
    conf --> fw
    exec_elf --> kern
    exec_elf --> fw
    exec_elf --> libkern
    exec_elf --> sys
    kpi --> fw
    kpi --> libkern
    kpi --> sys
    compat --> kern
    compat --> vm
    compat --> fw
    compat --> libkern
    compat --> sys
    ufs --> kern
    ufs --> fw
    ufs --> libkern
    ufs --> sys
    kern --> vm
    kern --> vt
    kern --> fw
    kern --> libkern
    kern --> sys
    vm --> fw
    vm --> sys
    vt --> fw
    vt --> libkern
    vt --> sys
    fw --> amd64
    fw --> riscv
    fw --> mdi
    fw --> sys
    fw --> libkern
    amd64 --> x86
    amd64 --> mdi
    amd64 --> sys
    amd64 --> libkern
    x86 --> sys
    x86 --> libkern
    mdi --> sys
    mdi --> libkern
    sys --> libkern
```

Rules of thumb:

* only `framework` names an architecture (`framework/arch/mod.rs`);
* only `conf` names an ABI (`compat/freebsd`) or a file system (`ufs`);
* services never depend on `mdi` or `arch/*` — they call `framework`;
* a driver will depend on `framework` and `freebsd_kpi` (no driver is
  built yet); `compat/*` never depends on either (R22/R23).

---

## 3. Boot: from `loader(8)` to `mi_startup`

```mermaid
sequenceDiagram
    autonumber
    participant L as FreeBSD loader(8)
    participant B as btext<br/>(arch/amd64/locore.rs)
    participant H as hammer_time<br/>(arch/amd64/machdep.rs)
    participant F as framework::__init
    participant M as mi_startup<br/>(kern/init_main.rs)

    L->>B: jump with long mode + paging on,<br/>modulep and kernend on the stack
    B->>B: reset rflags, switch to the boot stack
    B->>H: call hammer_time(modulep, kernend)
    H->>H: pcpu_init(0), COM1 early console, TSC calibrate
    H->>H: GDT + TSS + IDT (with exception stacks)
    H->>H: parse preload metadata (elf kernel, HOWTO, ENVP, EFI map)
    H->>H: pmap_bootstrap: kernel page tables + direct map
    H->>H: phys_avail = RAM - kernel - loader - hole - beyond DMAP
    H->>H: mask 8259, x2APIC init, ACPI MADT -> cpu_mp_setup
    H->>F: __nova_mi_startup(&BootInfo)
    F->>F: vm_phys::init(physavail)<br/>fb::init, task::init, smp::init
    F->>M: mi_startup(boot)
    M->>M: vt_init, kenv, params, banner, CPU report
    M->>M: sched_init(hz), initclocks(hz), inittimecounter
    M->>M: intr_enable, release_aps
    M->>M: self-tests (heap, kthreads, mutex, preemption)
    M->>M: vfs_mountroot (md_image + UFS)
    M->>M: start_init -> kthread "init" -> userret_loop
    M->>M: swapper(): the boot thread pauses forever
```

`kernel_entry!` is what makes the MD→MI jump type-safe: `conf/main.rs`
passes `kern::init_main::mi_startup`, the macro checks it against
`mdi::MiStartup`, defines `__nova_mi_startup`, and `hammer_time` calls
that symbol.

### `mi_startup` step by step

```mermaid
flowchart TD
    A["vt::vt_init — messages on the framebuffer"] --> B["StaticEnv, init_param1, panic wait time"]
    B --> C["linker_init — kernel symbols for backtraces"]
    C --> D["banner + CPU identification"]
    D --> E["cpu_startup — real/avail memory"]
    E --> F{"boot -v?"}
    F -->|yes| G["linker_preload — preloaded modules"]
    F -->|no| H["random_init (ChaCha20)"]
    G --> H
    H --> I["sched_init(hz) + initclocks(hz)"]
    I --> J["mp_start — print topology"]
    J --> K["inittimecounter"]
    K --> L["intr_enable"]
    L --> M["release_aps — start CPUs 1..N"]
    M --> N["self-tests"]
    N --> O["vfs_mountroot"]
    O --> P["kthread ttysig"]
    P --> Q["start_init"]
    Q --> R["swapper — boot thread pauses"]
```

---

## 4. Runtime: who talks to whom

```mermaid
flowchart LR
    subgraph U["user mode (ring 3)"]
        prog["FreeBSD ELF binary"]
    end

    subgraph K["kernel mode (ring 0)"]
        subgraph S["services — safe Rust"]
            sysent["compat/freebsd<br/>SysEntVec::syscall"]
            kfns["kern_* / vm_* / vfs_*<br/>policies"]
        end
        subgraph W["framework — trusted"]
            enter["user::enter / VmSpace"]
            trap["intr + trap upcalls"]
            task["task + scheduler"]
            mm["mm + sync"]
        end
        subgraph D["MD — trusted"]
            asm["alltraps, Xfast_syscall, iretq"]
            tables["GDT / IDT / TSS / pmap"]
        end
    end

    prog -->|"syscall instruction"| asm
    prog -->|"interrupt or fault"| asm
    asm --> sysent
    asm --> trap
    sysent --> kfns
    kfns --> W
    W --> D
    enter -->|"cpu_user_enter + iretq"| prog
    task -->|"cpu_switch"| task
```

The kernel never dereferences a user pointer: services take a `VmSpace`
and copy through `copyin`/`copyout`.

---

## 5. A system call

```mermaid
sequenceDiagram
    autonumber
    participant P as user program
    participant M as MD entry<br/>Xfast_syscall / alltraps_user
    participant L as kern::subr_trap::userret_loop
    participant A as compat/freebsd<br/>SysEntVec
    participant K as kern / vm service
    participant F as framework

    P->>M: syscall
    M->>M: swapgs, save user registers
    M-->>L: return from framework::user::enter
    L->>L: check pending signals first
    L->>A: abi.syscall(proc, regs)
    A->>A: decode number and args<br/>(%rdi %rsi %rdx %r10 %r8 %r9)
    A->>K: kern_write / kern_read / fork / ...
    K->>F: copyin/copyout, VmSpace, malloc, sleep
    F-->>K: owned data or error
    K-->>A: result or Errno
    A->>A: result in %rax, carry flag on error
    A-->>L: Return / Exit / Signal
    L->>F: user::enter(regs) on the next iteration
    F->>M: cpu_user_enter -> iretq
    M-->>P: back in user mode
```

`userret_loop` is plain Rust on the thread's kernel stack: entering user
mode is a function call that returns when the program comes back to the
kernel. There is no separate "return to user" path to audit.

---

## 6. Traps, interrupts, clock and preemption

```mermaid
sequenceDiagram
    autonumber
    participant H as hardware
    participant I as IDT gate
    participant A as alltraps
    participant T as amd64::trap
    participant F as framework::intr
    participant S as services

    H->>I: exception or interrupt
    I->>A: push error code (dummy if none) + FreeBSD T_* code
    A->>A: save registers
    A->>T: trap(&mut TrapFrame)

    alt exception in kernel
        T->>T: trap_fatal: FreeBSD report<br/>symbolized backtrace -> panic
    else clock / device interrupt
        T->>F: EOI, then __nova_intr(Interrupt)
        F->>S: clock_intr -> hardclock
        S->>S: sched_clock: slice over? request_preempt()
        T->>F: __nova_intr_exit
        F->>F: preempt_point -> mi_switch (exactly one guard)
        F->>F: cpu_switch(old, new)
    end
```

* `IPI_AST` → `request_preempt` on the target CPU.
* `IPI_STOP` → `cpu_halt` (the other CPUs stop when one panics).
* A thread can switch only with exactly one interrupt guard held; a
  kernel stack overflow hits an unmapped guard page and becomes a double
  fault on its own IST stack.

---

## 7. Threads and the pluggable scheduler

The framework owns the **mechanism**; `sched_4bsd` owns the **policy**.

```mermaid
flowchart LR
    subgraph FT["framework::task (trusted)"]
        spawn["spawn / yield_now / exit"]
        rq["run queue<br/>TaskQueue, links the threads"]
        pick["Scheduler::pick_next"]
        sw["mi_switch<br/>cpu_switch(old, new)"]
        wq["WaitQueue / sleep Mutex"]
        pre["preempt_point<br/>at interrupt exit"]
        spawn --> rq
        rq --> pick --> sw
        wq -->|wake| rq
        pre --> sw
    end

    subgraph S4["kern::sched_4bsd (safe policy)"]
        impl["impl Scheduler<br/>enqueue = sched_add<br/>pick_next = sched_choose"]
        clock["sched_clock<br/>slice = hz / 10 ticks"]
        clock -->|"slice expired"| pre
    end

    impl -. inject_scheduler .-> rq
    sw --> run["thread runs on a CPU"]
```

FreeBSD's names are kept on both sides: `sched_add`, `sched_choose`,
`sched_clock`, `mi_switch`, `kthread_add`, `pause`.

---

## 8. Process and userland lifecycle

```mermaid
flowchart TD
    start["start_init"] --> exec["kern_execve<br/>ImageActivator (exec/elf) -> abi_select,<br/>VmSpace, exec_setregs"]
    exec --> thread["kthread_add('init')<br/>subr_trap::userret_loop"]
    thread --> loop["userret_loop"]
    loop -->|"pending signal"| postsig["postsig -> handler frame, stop, or exit"]
    loop -->|"user::enter"| user["user mode"]
    user -->|syscall| dispatch["abi.syscall -> kern_* / vm_*"]
    dispatch --> loop
    user -->|"fault"| fault{"vm_fault_trap pages it in?"}
    fault -->|yes| loop
    fault -->|no| sigexit["abi.trapsignal -> sigexit"]
    postsig --> loop
    sigexit --> exit1["exit1: close fds, reparent children, zombie"]
    loop -->|"exit syscall"| exit1
    exit1 --> wait["parent: wait4 / wait6"]

    dispatch -->|fork| fork["kern_fork: VmSpace::fork, new Proc, new kthread"]
    fork --> loop
```

`init` is the first file of `init_path` that the loader preloaded or that
exists on the root file system. Its exit is fatal, as in FreeBSD:
`panic: Going nowhere without my init!`

---

## 9. VFS: the kernel knows no file system

```mermaid
flowchart LR
    sc["open / stat / read / getdirentries"]
    vfs["kern::vfs_syscalls<br/>kern::vfs_lookup::namei"]
    vn["kern::vnode::Vnode<br/>VfsOps"]
    ufs["ufs<br/>UFS2, read-only (sys/fs/freebsd/ufs)"]
    devfs["kern::devfs"]
    md["md_image preloaded by the loader"]
    conf["sys/conf"]

    sc --> vfs --> vn
    vn --> ufs
    vn --> devfs
    md --> vfs
    vfs -->|"vfs.root.mountfrom"| ufs
    vfs -->|"mount_devfs on the root"| devfs
    conf -->|"vfs_register(UFS_VFSCONF)"| vfs
```

The VFS works on vnodes; the root's type is a string
(`vfs.root.mountfrom`), and `conf` is the only place that names UFS.

---

## 10. Memory

```mermaid
flowchart TD
    phys["BootInfo::physavail<br/>usable RAM from the loader"]
    buddy["framework::mm::vm_phys<br/>buddy allocator (libkern::buddy)"]
    frame["Frame / Segment<br/>owned pages: zeroed on alloc, freed on drop"]
    kmem["mm::kmem<br/>slab classes 16 B – 2 KiB + buddy blocks"]
    malloc["mm::malloc (malloc(9))<br/>MallocType + M_WAITOK / M_NOWAIT -> Result"]
    owners["KBox / KVec / KArc / KFnOnce"]
    kstack["KernelStack<br/>guard page"]
    vmspace["user::VmSpace<br/>address space, copyin/copyout"]
    dmap["direct map<br/>phys_to_dmap, DMAP at 0xfffff80000000000"]

    phys --> buddy
    buddy --> frame
    buddy --> kmem
    buddy --> kstack
    buddy --> dmap
    kmem --> malloc --> owners
    frame --> vmspace
    dmap -. free-page links live in the pages .-> buddy
```

There is **no global allocator and no `alloc` crate** (rule R16): `Box`,
`Vec` and `format!` do not exist. Every allocation names its owner and
whether it may sleep, and returns an error instead of panicking. Frames
are read and written by copying, never through a `&[u8]`, so userland
can never invalidate a kernel reference.

---

## 11. Starting the application processors

```mermaid
sequenceDiagram
    autonumber
    participant B as BSP (CPU 0)
    participant F as framework::smp
    participant M as amd64 mp_machdep
    participant A as AP

    B->>B: cpu_mp_setup reads ACPI MADT (XSDT -> MADT)
    B->>F: start_ap(n)
    F->>F: allocate idle thread, boot stack, 3 exception stacks
    F->>M: cpu_start_ap(n, stack_top, exception stacks)
    M->>M: copy mpboot to 0x8000, INIT-SIPI-SIPI
    M->>A: real -> protected -> long mode
    A->>A: init_secondary: kernel CR3, %gs, GDT/TSS/IST, x2APIC
    A->>F: __nova_ap_startup(n)
    F->>F: clock, mp_ncpus_inc, online, ap_enter(idle)
    Note over A: idle thread sleeps in hlt;<br/>IPI_AST wakes it to reschedule
```

Only the boot CPU's clock advances `ticks`; the local APIC timer is
calibrated once and reused by the APs.

---

## 12. Build and verification

```mermaid
flowchart LR
    make["make"] --> check["make check"]
    make --> build["cargo build --release<br/>x86_64-unknown-none"]
    make --> userland["userland: hello / hello_libc"]
    make --> stand["stand.efi<br/>x86_64-unknown-uefi"]

    check --> layers["tools/checklayers<br/>rules R1–R23"]
    check --> riscv["cargo check for riscv64<br/>proof that MI is MI"]
    layers --> build
    riscv --> build
    build --> kernel["sys/target/.../kernel"]
    kernel --> qemu["make qemu"]
    kernel --> bhyve["make bhyve<br/>(FreeBSD host, root)"]

    make --> test["make test<br/>host unit tests"]
    make --> lint["make clippy"]
    make --> miri["make miri<br/>UB detection"]
    make --> kani["make kani<br/>proofs of libkern/sys/x86"]

    qemu --> selftest["SELFTEST=pf | stack<br/>fatal traps on purpose"]
    bhyve --> verbose["VERBOSE=1<br/>heap, kthread, mutex,<br/>preemption self-tests"]
```

The kernel that violates the layering is never built: `make check` runs
before `cargo build`.

---

## 13. Where the diagrams stop

The tree already contains the boot path (milestone 0–3b), user address
spaces and user mode (milestone 4), and the userland-facing work of
milestone 5: FPU/SSE per thread, demand paging, `mmap`, signals with
handlers and `sigreturn`, TLS, sysctl, the console line discipline and
`TIOCSPGRP`, descriptors, VFS with read-only UFS2 and devfs, processes,
`fork`/`execve`/`wait`, and pipes. That is why the diagrams show `init`
running and the syscall loop, not a kernel that halts before userland.

The diagrams deliberately do not show what is still on the roadmap:

* the full `mmap` surface, the sysctls `ld-elf.so.1` reads, and dynamic
  binaries — milestone 6;
* `bus_space`/`bus_dma`, PCI, virtio-blk — milestone 7;
* loadable modules — milestone 7b;
* the network stack — milestone 8;
* TLB shootdown IPIs, NMI stop, per-CPU run queues (`sched_ule`), xAPIC;
* real hardware (today: bhyve/QEMU, and x2APIC only).

Each of them enters the picture at an already-drawn boundary: a driver
becomes another safe service calling `framework`; a scheduler replaces
`Sched4bsd`; an ABI becomes another `compat/<abi>` registered by `conf`.

The boundaries the diagrams do draw are enforced, not just documented:

* **Executable format:** the core knows `ImageFile` and
  `kern_exec::ImageActivator`; `sys/exec/elf` is a service, registered by
  `conf`, and rule R20 keeps `Ehdr`/`PT_LOAD` out of `kern`. A new format
  is a new crate under `sys/exec/`, with no core change.
* **ABI:** `SysEntVec` is split into `ExecAbi`, `SyscallAbi` and the
  optional `SignalAbi`; a personality with no signal model returns `None`
  from `signals()`. The POSIX signal mechanism is still `kern_sig` in the
  kernel core: moving it into a personality needs a generic `ProcEvent`
  layer (the remaining signal-isolation step).
* **First process:** `InitPolicy` (default `init_path`, the `-s` of
  single-user mode) is registered by `conf`; `kern` no longer spells
  FreeBSD's userland out.
* **File system:** UFS is `sys/fs/freebsd/ufs`, not under
  `compat/freebsd`; rule R19 keeps file system names out of the core,
  and a file system is not an ABI.
