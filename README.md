# Nova Project

[![CI](https://github.com/Renan2010p/nova/actions/workflows/ci.yml/badge.svg)](https://github.com/Renan2010p/nova/actions/workflows/ci.yml)

**The kernel of the Renux "Nova" Project.**

Nova is a kernel written 100% in Rust, licensed under the MPL-2.0, that
aims to be a **drop-in replacement for the FreeBSD 15.1 kernel**: the
FreeBSD userland (libc, `ld-elf.so.1`, `/bin/sh`, `ifconfig`, ...) and
FreeBSD packages must run on Nova unmodified.

The goal is for Renux to stop depending on FreeBSD, one component at a
time: first the kernel (Renux "Nova"), then the bootloader (Nova stand),
then the userland.

```
              today                Renux "Nova" (phase 1)     Renux "Nova" (final)
 userland     FreeBSD              FreeBSD                    Nova userland
 kernel       FreeBSD              Nova  <-- this repository  Nova
 boot         FreeBSD loader(8)    FreeBSD loader(8)          Nova stand
```

## Design principles

1. **Framekernel.** A small trusted *framework* (`sys/framework`,
   `sys/arch/*`, `sys/libkern`) is the only code allowed to use `unsafe`,
   and every API it exposes is sound. Everything else, the *services*
   (`kern`, `vm`, `fs`, `dev`, `compat`, ...), is 100% safe Rust
   (`#![forbid(unsafe_code)]`). A memory-safety bug can only live in the
   framework.
2. **Rigorous MD/MI separation**, NetBSD style. Machine-dependent code
   lives in `sys/arch/`; machine-independent code cannot reach it except
   through the framework. This is checked by a tool, not by convention.
3. **SMP from day one.** Per-CPU data, CPU identity and interrupt-safe
   locks are part of the framework's API before there is a second CPU to
   run them. Services are written to be correct on any number of CPUs.
4. **The FreeBSD ABI is a personality, not the kernel.** FreeBSD binaries
   run natively, but syscall numbers, struct layouts, errno and sysctl
   names all live in `sys/compat/freebsd/`. The rest of the kernel does
   not know which userland it is running.
5. **BSD names.** `sys/kern/kern_fork.rs`, `sys/vm/vm_map.rs`,
   `sys/arch/amd64/machdep.rs`, ... Anyone who knows FreeBSD finds their
   way around Nova.

## Status: milestones 0–5e — FreeBSD userland

The kernel is loaded by **FreeBSD's `loader(8)`** and takes the machine
over: its own GDT/IDT, page tables and direct map, a page allocator and
heap, the local APIC clock, SMP start-up, and preemptive kernel threads
with a pluggable scheduler (4BSD), sleep mutexes and wait queues. Fatal
traps and panics are reported exactly like FreeBSD reports them, with a
symbolized backtrace.

On top of that it runs FreeBSD ELF binaries: user address spaces and user
mode, an ELF image activator, the FreeBSD personality (syscalls, errno,
signals, the initial stack), demand paging and `mmap`, the console line
discipline, a VFS with read-only UFS2 and devfs, processes
(`fork`/`execve`/`wait`), signals and pipes. The code boots to
`/rescue/sh` from a UFS root under bhyve/QEMU (to be validated on each
host); the remaining milestones are the full `mmap` and dynamic binaries
(6), drivers (7), modules (7b) and the network (8).

The kernel core knows no ABI, no executable format and no file system:
those are services registered by `sys/conf`. See
[docs/DIAGRAM.md](docs/DIAGRAM.md) for the boundaries.

The console reads like FreeBSD's: the same boot messages (`CPU:`,
`Features=`, `real memory`, `avail memory`, `Event timer`...), the same
panic report and the same halt and reboot prompts.

## Building and running

Requirements: Rust (the version is pinned in `rust-toolchain.toml`;
`rustup` installs it automatically) and `make` (BSD or GNU).

```sh
make              # check the layering rules, then build
                  # sys/target/x86_64-unknown-none/release/kernel
make check        # layering rules only (tools/checklayers)
make test         # unit tests of the crates that run on the host
doas make bhyve   # on FreeBSD, as root: boot it in bhyve with the system loader
doas make bhyve VERBOSE=1        # boot -v: diagnostics and self-test results
doas make bhyve SELFTEST=pf      # a fatal page fault on purpose (also: stack)
```

See [docs/BOOT.md](docs/BOOT.md) for the boot protocol and testing.

## Documentation

| Document | Contents |
|---|---|
| [docs/DIAGRAM.md](docs/DIAGRAM.md) | diagrams: boot chain, layers, syscalls, traps, threads, memory |
| [docs/DRIVERS.md](docs/DRIVERS.md) | proposal: native, FreeBSD and Linux drivers, KPI shims, userspace host |
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | overview, the `sys/` tree, framekernel, MD/MI, SMP |
| [docs/LAYERING.md](docs/LAYERING.md) | the layering rules and how they are enforced |
| [docs/BOOT.md](docs/BOOT.md) | the contract with `loader(8)` and how to test under bhyve |
| [docs/STYLE.md](docs/STYLE.md) | license header, naming, comments, `unsafe` |
| [docs/ROADMAP.md](docs/ROADMAP.md) | milestones |
| [stand/README.md](stand/README.md) | the future Nova stand (bootloader) |

## License

Copyright (c) 2026 Renan Lucas Vieira Hilário.
Licensed under the [Mozilla Public License 2.0](LICENSE). See also
[COPYRIGHT](COPYRIGHT).

FreeBSD is a registered trademark of The FreeBSD Foundation. Nova is
compatible with FreeBSD; it is not FreeBSD and is not affiliated with The
FreeBSD Foundation.
