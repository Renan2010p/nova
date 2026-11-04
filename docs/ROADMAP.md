# Roadmap

Each milestone ends with something you can boot and see. Scope stays
narrow on purpose (amd64, bhyve/QEMU, virtio) until the kernel runs the
FreeBSD userland; real hardware comes after.

| # | Milestone | Done when | Status |
|---|---|---|---|
| 0 | Workspace, framekernel layering, `checklayers`, pinned toolchain | `make check` passes | **done** |
| 1 | Boot via FreeBSD `loader(8)`: `btext`, `hammer_time`, per-CPU area, COM1, preload metadata, memory map | banner and memory map printed under bhyve | **done** |
| 2 | GDT/IDT/TSS, traps with FreeBSD's `trap_fatal`; kernel page tables and direct map; framework: `vm_phys`, `Frame`/`Segment`, kernel heap; x2APIC timer, `hardclock` | a page fault is caught and reported, `Box` works, the clock ticks | **done** |
| 3 | Threads: context switch, kernel stacks with guard pages (`pmap_kenter`/`pmap_kremove`), injected scheduler (4BSD round robin), preemption, wait queues, sleep mutexes, `pause`; FreeBSD-style boot, panic and halt messages with symbolized backtraces | kernel threads take turns, a busy thread is preempted, mutex contention is correct | **done** |
| 3b | **SMP**: CPUs from the ACPI MADT, AP start-up (`mpboot`, INIT-SIPI-SIPI, `init_secondary`), per-CPU idle threads and clocks, `IPI_AST` and `IPI_STOP`; FreeBSD's SMP messages | `Launching APs: 1`, the self-tests pass on every CPU | **done** (to be validated under bhyve) |
| 3b+ | TLB shootdown IPIs, NMI stop, per-CPU run queues (ULE) | kernel addresses can be recycled | |
| 3c | **Second architecture stub** (`sys/arch/riscv64`, compile-only) and rule R13 (no MD vocabulary in MI code) | `make check` builds the kernel for riscv64 | **done** |
| 4 | User address spaces (`VmSpace` over `pmap`); user mode; ELF loader; `compat/freebsd` `sysentvec`; `copyin`/`copyout`; rule R15 (the kernel knows no ABI) | a static FreeBSD binary does `write` + `exit` | **done** (to be validated under bhyve) |
| 4b | Formal verification, phase 1 (see [VERIFICATION.md](VERIFICATION.md)): Miri on the host tests, Kani proofs of `libkern` and the checks guarding `unsafe` | `make miri` and `make kani` pass | **done** |
| 5 | VFS, devfs, tmpfs/`md_image` root; `fork`/`exec`/`wait`; signals | `/rescue/sh` runs | |
| 6 | Full `mmap`; the sysctls libc needs; `ld-elf.so.1` | dynamic FreeBSD binaries run | |
| 7 | `bus_space`/`bus_dma`, PCI, virtio-blk, UFS | boots to `init` from a FreeBSD disk | |
| 7b | Loadable kernel modules (kld): the kernel linked with `.dynamic` and a `DT_HASH` symbol table as FreeBSD's is, so loader(8) finds `kernphys`, `__start_set_modmetadata_set` and friends (today it prints `elf64_lookup_symbol: corrupt symbol table` and carries on); module metadata; preloaded and `kldload`ed modules | the loader is silent, a preloaded module initializes | |
| 8 | Network (virtio-net, TCP/IP) | `fetch` works | |
| 9+ | FreeBSD test suite (`/usr/tests`, kyua) as the compatibility score; real hardware; Renux "Nova" | | |

Tools planned along the way:

* `tools/makesyscalls`: generates `compat/freebsd/freebsd_syscalls.rs`
  from FreeBSD's `sys/kern/syscalls.master` (**done**, `make sysent`).
* `tools/abidump`: a C program run on FreeBSD that prints `sizeof` and
  `offsetof` of every ABI struct, which the personality checks against
  (milestone 4).
* A QEMU runner next to the bhyve one, booting a small FreeBSD-style disk
  image made with `makefs` + `mkimg`.
