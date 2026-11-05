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
| 4c | **Explicit code**, as in Zig: no hidden allocation (`malloc(9)` with `MallocType` and flags, `KBox`/`KVec`/`KArc`/`KFnOnce`, no `alloc` crate, rule R16), one crate per subsystem (R17), no `as` or `unwrap()` (R18) | the kernel links no allocator; `make clippy` is clean on every port | **done** (to be validated under bhyve) |
| 4d | Console on the UEFI framebuffer, as FreeBSD's `vt(4)`/`efifb` (white text on the screen), with the font the loader passes (`MODINFOMD_FONT`); tested with QEMU and `edk2` | a UEFI boot shows the kernel's messages on the screen | **done** (to be validated under bhyve) |
| 5a | **libc programs**: FPU/SSE state per thread; demand-zero paging (`vm_fault`) over `VmSpace` regions; anonymous `mmap`/`munmap`; signal masks and actions (`sigprocmask`, `sigaction`); TLS (`sysarch(AMD64_SET_TLSBASE)`); `clock_gettime`, `thr_self`, `issetugid`; FreeBSD's `uprintf_signal` report | a static program built with FreeBSD's `cc` and libc prints with `printf` | **done** (to be validated under bhyve) |
| 5b | Kernel random generator (ChaCha20, fast key erasure, seeded by `rdseed`/`rdrand`; random stack canary); FreeBSD `sysctl` and `sysctlbyname` for what libc reads (`kern.arandom`, `hw.ncpu`, `hw.pagesize`, `kern.osreldate`, ...); `getrandom`, `minherit`, `thr_kill` to oneself, `cpuset_getaffinity` | `/rescue/sh` starts up and reads its first command | **done** (to be validated under bhyve) |
| 5c | Descriptor tables; the console terminal with FreeBSD's line discipline (canonical editing, echo, `termios`, `winsize`) and serial input; `read`, `close`, `ioctl`, `fstat`, `fstatat`, `__getcwd`, `mprotect`, credentials; `tools/abidump` checks the FreeBSD structures | `/rescue/sh` takes commands typed on the console | **done** (to be validated under bhyve) |
| 5d | **VFS**, knowing no file system: vnodes behind `kern::vnode::VfsOps`, `namei` with symbolic links, mounting the root from `vfs.root.mountfrom` on the memory disk the loader preloads (`md_image`); UFS2, read-only, in `sys/fs/freebsd/ufs`, registered by `conf`; rule R19 (the kernel knows no file system); `open`, `fstatat`, `getdirentries`, `readlink`, `chdir`, `__getcwd`, `access`, `lseek`, `dup`/`fcntl`, `statfs`, `pathconf`, `readv`/`writev` | `/rescue/sh` starts from a UFS root; `ls`, `cat`, `df` work | **done** (to be validated under bhyve) |
| 5e | **Processes**: process table, `fork` (address space copied as `minherit` says), `execve` from the file system, zombies, `wait4`/`wait6`, reparenting to init, process groups and sessions, `kill` (default actions), `nanosleep`, `kern.proc.pathname` | `/rescue/sh` runs programs and gets their exit status | **done** (to be validated under bhyve) |
| 5f | Pipes; signal delivery to handlers (`sendsig`, `sigreturn`, FPU state in the frame); interruptible sleeps (`EINTR`, `ERESTART`); stop and continue, `wait` with `WUNTRACED`/`WCONTINUED`; sessions, the console as the controlling terminal (`TIOCSCTTY`, `TIOCSPGRP`, `^C` `^\` `^Z`); devfs on `/dev`; the message buffer (`dmesg`) | `ls \| head` in `/rescue/sh`; with `SINGLE=1 INIT_PATH=/rescue/init`, FreeBSD's single-user shell with job control (`^C`, `^Z`, `jobs`, `bg`, `kill %1`) | **done** (QEMU; to be validated under bhyve) |
| 6 | Full `mmap`; the sysctls libc needs; `ld-elf.so.1` | dynamic FreeBSD binaries run | |
| 7 | `bus_space`/`bus_dma`, PCI, virtio-blk, UFS | boots to `init` from a FreeBSD disk | |
| 7b | Loadable kernel modules (kld): the kernel linked with `.dynamic` and a `DT_HASH` symbol table as FreeBSD's is, so loader(8) finds `kernphys`, `__start_set_modmetadata_set` and friends (today it prints `elf64_lookup_symbol: corrupt symbol table` and carries on); module metadata; preloaded and `kldload`ed modules | the loader is silent, a preloaded module initializes | |
| 8 | Network (virtio-net, TCP/IP) | `fetch` works | |
| 9+ | FreeBSD test suite (`/usr/tests`, kyua) as the compatibility score; real hardware; Renux "Nova" | | |

## The Nova stand (bootloader)

| # | Milestone | Done when | Status |
|---|---|---|---|
| S1 | `stand.efi` in Rust (`stand/`), a loader that knows no system: `stand.conf` (Limine's syntax) and a menu laid out as Limine's; protocols `freebsd` (FreeBSD's `loader.efi`: environment, preload metadata, amd64 trampoline), `linux` (EFI stub, initrd), `efi` (chainload) | it boots FreeBSD 15.1's own kernel to `mountroot>`, the Nova kernel, Alpine Linux, and FreeBSD's `loader.efi` | **done** (QEMU with edk2) |
| S2 | Modules for `freebsd` (`module_path`, `*_load`: Nova's init, FreeBSD's `.ko`), symbols for DDB, `loader.conf.d` | FreeBSD boots with modules; Nova's init loads | |
| S3 | Other volumes and filesystems (UFS, shared with the kernel; ext4); sub-entries and the entry editor | boots from a FreeBSD disk | |
| S4 | BIOS path (`gptboot`, `loader`) and `userboot` for bhyve | bhyve boots with the Nova stand | |

Tools planned along the way:

* `tools/makesyscalls`: generates `compat/freebsd/freebsd_syscalls.rs`
  from FreeBSD's `sys/kern/syscalls.master` (**done**, `make sysent`).
* `tools/abidump`: a C program run on FreeBSD that prints `sizeof` and
  `offsetof` of ABI structures and the values of ABI constants, which the
  personality checks against (**done** for `stat`, `termios`, `winsize`
  and the terminal `ioctl`s; `make abidump`).
* A QEMU runner next to the bhyve one, booting a small FreeBSD-style disk
  image made with `makefs` + `mkimg`.
