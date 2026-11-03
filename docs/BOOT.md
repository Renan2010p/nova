# Booting Nova

Nova is booted by **FreeBSD's `loader(8)`**, unmodified: BIOS `loader`,
`loader.efi`, or bhyve's `userboot` (through `bhyveload`). The future
[Nova stand](../stand/README.md) will speak the same protocol.

Everything below was checked against the FreeBSD 15.1 sources
(`stand/common/load_elf.c`, `stand/i386/libi386/bootinfo64.c`,
`stand/userboot/userboot/elf64_freebsd.c`,
`stand/efi/loader/arch/amd64/elf64_freebsd.c`,
`sys/amd64/amd64/locore.S`, `sys/amd64/amd64/machdep.c`).

## What the loader expects from the kernel file

| Requirement | Nova |
|---|---|
| ELF64, little endian, `EM_X86_64`, `ET_EXEC` (the OS/ABI byte is not checked) | yes |
| Entry within the first 16 MiB of `KERNBASE`; the loader places each `PT_LOAD` at `p_vaddr - (e_entry & 0xffffffffff000000)` | linked at `0xffffffff80200000`, so loaded at physical 2 MiB |
| `PT_DYNAMIC` (optional: used to find `modmetadata` and `kernphys`; if present, `__start_set_modmetadata_set` becomes mandatory) | absent: symbol lookups fail harmlessly (see "Testing under bhyve") |
| No `kernphys` symbol, so not physically relocatable | the EFI loader copies the kernel to 2 MiB ("copy staging") |

`.bss` is zero-filled by the loader (`kern_bzero`). The layout is fixed by
`sys/conf/ldscript.amd64`, which must agree with
`sys/arch/amd64/vmparam.rs` (`KERNBASE`, `KERNLOAD`).

## State at entry (`btext`)

* 64-bit long mode, on a 64-bit code segment, interrupts disabled;
* paging on, physical `[0, 1 GiB)` mapped at `KERNBASE`
  (`0xffffffff80000000`) with 2 MiB pages;
* a scratch stack with three 32-bit words:

```
%rsp + 0   return address (unused)
%rsp + 4   modulep   physical address of the preload metadata
%rsp + 8   kernend   physical end of kernel + modules + metadata + environment
```

## What Nova does

1. **`btext`** (`arch/amd64/locore.rs`): resets `%rflags`, reads `modulep`
   and `kernend`, switches to the 64 KiB boot stack, calls `hammer_time`.
   This is the same sequence as FreeBSD's `locore.S`.
2. **`hammer_time`** (`arch/amd64/machdep.rs`):
   1. `pcpu_init(0)`: `MSR_GSBASE` now points at CPU 0's per-CPU area;
   2. initializes COM1 (115200 8N1) as the early console;
   3. takes `[KERNBASE + modulep, KERNBASE + kernend)` as the preload area
      (the same formula FreeBSD uses for `preload_metadata`);
   4. finds the kernel's records (`MODINFO_TYPE` = `"elf kernel"`) and reads
      `MODINFOMD_HOWTO` and `MODINFOMD_ENVP`;
   5. builds the physical memory map from `MODINFOMD_EFI_MAP` (UEFI) or
      `MODINFOMD_SMAP` (BIOS/userboot), sorted and coalesced
      (`sys::physmem::PhysMap`);
   6. calls `mi_startup` with an MI `BootInfo`.
   Before the memory map, it also loads the kernel's own GDT (FreeBSD's
   layout), TSS (with IST stacks for double faults, NMIs and machine
   checks) and IDT. After it, `pmap_bootstrap` replaces the loader's page
   tables with the kernel's (KERNBASE plus the direct map), `phys_avail` is
   computed, and the local APIC is switched to x2APIC mode.
3. **The framework** builds the page allocator from `phys_avail` (from
   here on the kernel heap works), makes the boot code thread 0
   (`swapper`) and creates the idle thread (`idle: cpu0`).
4. **`mi_startup`** (`kern/init_main.rs`), printing what FreeBSD prints at
   each step: the copyright and version, the CPU report, the memory sizes;
   then it installs the 4BSD scheduler, starts the clock at `kern.hz`
   (default 1000, or 100 under a hypervisor, as in FreeBSD), enables
   interrupts, runs the self-tests (heap, kernel threads, sleep mutexes,
   preemption), and halts where FreeBSD would start `init(8)`.

## Preload metadata

A list of records, each `u32 type | u32 size | data`, padded to 8 bytes.
A file's records start at `MODINFO_NAME`; the list ends with type 0, size
0. MI record types are in `sys/sys/linker.rs`, amd64 ones in
`sys/arch/amd64/metadata.rs`. The parser is bounds-checked and
unit-tested (`make test`): a corrupted list stops parsing, it does not
crash the kernel.

The loaders lay memory out like this, all physical. The BIOS and EFI
loaders put the environment *after* the metadata, userboot (`bhyveload`)
puts it *before*, so Nova reads the environment from `[envp, kernend)` and
stops at its terminating empty string:

```
              BIOS / EFI                         userboot
0x200000      kernel image (text, rodata,        kernel image
              data, bss)
...           preloaded modules                  preloaded modules
              modulep: preload metadata          envp: environment
              envp: environment                  modulep: preload metadata
kernend       (page aligned)                     (page aligned)
```

The environment is `"name=value\0...name=value\0\0"`.

Nova reports `[KERNLOAD, kernend)` as `BootInfo::kernel`. The VM system
must never hand out that range.

## Testing under bhyve

On the FreeBSD host, from a native shell (not inside the Linuxulator), as
root:

```sh
make                 # as your user
doas make bhyve      # or: doas sh tools/bhyve/run.sh sys/target/x86_64-unknown-none/release/kernel
```

`tools/bhyve/run.sh` builds a throw-away `/boot` (the host's
`/boot/lua` and `/boot/defaults`, plus Nova as `/boot/kernel/kernel`), runs
`bhyveload -h` on it, then `bhyve -x` with COM1 on your terminal. Nova
needs x2APIC for now (`-x` offers it to the guest). Expected output,
after the loader's own messages (the CPU lines depend on your machine):

```
Copyright (c) 2026 Renan Lucas Vieira Hilário.
Nova Project, the kernel of the Renux "Nova" Project.
Licensed under the Mozilla Public License, v. 2.0.
Nova 0.1.0-CURRENT main-n10-0123456789ab GENERIC amd64
rustc 1.99.1 (58cf8f9c3 2026-10-13)
CPU: Intel(R) Core(TM) i7-8750H CPU @ 2.20GHz (2208.00-MHz K8-class CPU)
  Origin="GenuineIntel"  Id=0x906ea  Family=0x6  Model=0x9e  Stepping=10
  Features=0x...<FPU,VME,DE,PSE,TSC,MSR,PAE,MCE,CX8,APIC,...>
  Features2=0x...<SSE3,...,x2APIC,...,HV>
  AMD Features=0x...<SYSCALL,NX,Page1GB,RDTSCP,LM>
  ...
Hypervisor: Origin = "bhyve bhyve "
real memory  = 536870912 (512 MB)
avail memory = 533331968 (508 MB)
Event timer "LAPIC" quality 600
ACPI APIC Table: <BHYVE  BVMADT  >
Nova/SMP: Multiprocessor System Detected: 2 CPUs
Nova/SMP: 2 package(s) x 1 core(s)
Timecounters tick every 10.000 msec
Launching APs: 1
Nova: no userland yet; halting.
Uptime: 1s
The operating system has halted.
Please press any key to reboot.

Rebooting...
run.sh: guest rebooted
```

Press a key to reboot (bhyve then exits). With `VERBOSE=1` (the loader's
`boot_verbose`, i.e. `boot -v`) the physical memory chunks, the preloaded
files and the self-test results are shown too:

```sh
doas make bhyve VERBOSE=1
```

```
selftest: kmem: Box and Vec work (2 pages kept by the heap's slabs)
selftest: kthreads: tids [100002, 100003] took turns: ping pong ping pong ping pong
selftest: mutex: 3 threads x 500 increments = 1500
selftest: preemption: a busy thread was preempted after 10 ticks (quantum 10)
```

To check that faults are caught, boot with `SELFTEST=pf` (a kernel page
fault) or `SELFTEST=stack` (a kernel stack overflow, caught by the guard
page as a double fault):

```sh
doas make bhyve SELFTEST=pf
```

The kernel reports it the way FreeBSD does, prints a backtrace with
function names that crosses the trap frame into the faulting code
(`--- trap 0xc, ...`, as FreeBSD's `db_backtrace`), and reboots after 15
seconds unless a key is pressed (`kern.panic_reboot_wait_time`). With
`SELFTEST=stack` the double fault's backtrace goes on from the IST stack
into the overflowed thread's stack, through the `recurse` frames:

```
selftest: taking a page fault on purpose


Fatal trap 12: page fault while in kernel mode
cpuid = 0; apic id = 00
fault virtual address	= 0xdeadbeef
fault code		= supervisor read data, page not present
instruction pointer	= 0x20:0xffffffff802...
...
trap number		= 12
panic: page fault
  at arch/amd64/trap.rs:...
cpuid = 0
time = 1
KDB: stack backtrace:
#0 0xffffffff802... at kern::kern_shutdown::kdb_backtrace+0x...
#1 0xffffffff802... at __rustc::rust_begin_unwind+0x...
#2 0xffffffff802... at core::panicking::panic_fmt+0x...
#3 0xffffffff802... at amd64::trap::trap_fatal+0x...
#4 0xffffffff802... at amd64::trap::trap+0x...
#5 0xffffffff802... at calltrap+0x9
--- trap 0xc, rip = 0xffffffff802..., rsp = 0xffffffff802..., rbp = 0xffffffff802... ---
#6 0xffffffff802... at <amd64::Machine as mdi::Cpu>::cpu_selftest_fault+0x...
#7 0xffffffff802... at framework::cpu::cpu_selftest_fault+0x...
#8 0xffffffff802... at kern::subr_selftest::trap_selftest+0x...
#9 0xffffffff802... at kern::init_main::mi_startup+0x...
...
Uptime: 1s
Automatic reboot in 15 seconds - press a key on the console to abort
```

`boothowto` is `0x1000` (`RB_SERIAL`, from `console="comconsole"`). If
it also has `0x10000` (`RB_MUTE`), `boot_mute="YES"` is set somewhere;
Renux sets it in `/boot/defaults/loader.conf`, which is why `run.sh` forces
it off. Nova does not honour `RB_MUTE` yet (see `kern/kern_cons.rs`).

Under `userboot`, the memory map is reported as "BIOS" because
`bhyveload` passes an SMAP.

The loader also prints `elf64_lookup_symbol: corrupt symbol table`. This
is expected and harmless. The loader looks up the `kernphys` symbol to
decide whether the kernel is physically relocatable. Nova has no
`PT_DYNAMIC`, so the lookup fails with that message, and the loader reaches
the right answer (not relocatable) and goes on. Silencing it means giving
Nova a dynamic symbol table, and then the loader *requires* a
`modmetadata` linker set (FreeBSD's `MODULE_VERSION(kernel, ...)`), or it
refuses the kernel. That set will come when Nova needs it. If the output stops before the banner, try
`boot -v` at the loader prompt, and check the loader's message
`/boot/kernel/kernel text=... data=...`: the sizes should match
`readelf -l` of the kernel.

### On real hardware

Copy the kernel to `/boot/nova/kernel` and boot it from the loader prompt
with `boot nova` (or `kernels="kernel nova"` in `loader.conf` to pick it
from the menu). Nova only prints to COM1 and halts, so you need a serial
console to see anything, and you reboot by hand. Your FreeBSD kernel in
`/boot/kernel` is not touched.
