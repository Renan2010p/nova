# Roadmap

Each milestone ends with something you can boot and see. Scope stays
narrow on purpose (amd64, bhyve/QEMU, virtio) until the kernel runs the
FreeBSD userland; real hardware comes after.

| # | Milestone | Done when | Status |
|---|---|---|---|
| 0 | Workspace, framekernel layering, `checklayers`, pinned toolchain | `make check` passes | **done** |
| 1 | Boot via FreeBSD `loader(8)`: `btext`, `hammer_time`, per-CPU area, COM1, preload metadata, memory map | banner and memory map printed under bhyve | **done** (to be validated under bhyve) |
| 2 | GDT/IDT/TSS, traps; framework: `vm_phys` frame allocator, kernel heap, `VmSpace` over `pmap`; local APIC timer | a page fault is handled and `Box` works | |
| 3 | Threads: context switch, kernel stacks, scheduler, sleep locks | two kernel threads alternate | |
| 3b | **SMP**: AP start-up (`mp_machdep`, INIT-SIPI-SIPI), IPIs, TLB shootdown, per-CPU run queues | all CPUs run kernel threads; `CPUs: N running` | |
| 3c | **Second architecture stub** (riscv64 or aarch64) that compiles | proves no MD code leaked into MI code | |
| 4 | User mode; ELF loader; `compat/freebsd` `sysentvec`; `UserPtr`/`copyin`/`copyout` | a static FreeBSD binary does `write` + `exit` | |
| 5 | VFS, devfs, tmpfs/`md_image` root; `fork`/`exec`/`wait`; signals | `/rescue/sh` runs | |
| 6 | Full `mmap`; the sysctls libc needs; `ld-elf.so.1` | dynamic FreeBSD binaries run | |
| 7 | `bus_space`/`bus_dma`, PCI, virtio-blk, UFS | boots to `init` from a FreeBSD disk | |
| 8 | Network (virtio-net, TCP/IP) | `fetch` works | |
| 9+ | FreeBSD test suite (`/usr/tests`, kyua) as the compatibility score; real hardware; Renux "Nova" | | |

Tools planned along the way:

* `tools/makesyscalls`: generates `compat/freebsd/freebsd_sysent.rs` from
  FreeBSD's `sys/kern/syscalls.master` (milestone 4).
* `tools/abidump`: a C program run on FreeBSD that prints `sizeof` and
  `offsetof` of every ABI struct, which the personality checks against
  (milestone 4).
* A QEMU runner next to the bhyve one, booting a small FreeBSD-style disk
  image made with `makefs` + `mkimg`.
