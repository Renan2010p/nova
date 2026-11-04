# Layering rules

Nova's architecture holds up only if its boundaries hold. These rules are
not guidelines: `tools/checklayers` enforces R1–R9 on every build
(`make` runs `make check` first), and the compiler enforces the rest.

## Layers

A crate's layer is decided by its path in `sys/`:

| Layer | Crates | `unsafe` | May depend on |
|---|---|---|---|
| **base** | `libkern`, `sys`, `mdi` | only `libkern` (`mdi` may *declare* `unsafe trait`s) | base |
| **md** | `arch/*` | yes | base, md |
| **framework** | `framework` | yes | base, md |
| **service** | everything else (`kern`, `vm`, `compat/*`, `fs/*`, `dev/*`, ...) | **forbidden** | `libkern`, `sys`, framework, service |
| **conf** | `conf` | denied (the entry macro opts in) | `libkern`, `sys`, framework, service |

The **trusted base** (the framekernel's TCB) is `libkern`, `arch/*` and
`framework`. A new directory is a service by default, the strictest layer.

## Rules

| Rule | Statement | Enforced by |
|---|---|---|
| **R1** | Dependencies only point to the layers in the table. In particular, only `framework` (and other MD crates) depend on `arch/*`, and `arch/*` never depends on the framework or on services. | checklayers |
| **R2** | Services and `conf` do not depend on `mdi`; they use the framework's wrappers. | checklayers |
| **R3** | `cfg(target_arch …)` appears only in `sys/arch/*` or in files under an `arch/` directory: `framework/arch/mod.rs` (the one place that selects the port) and each ABI's `compat/<abi>/arch/<arch>/`. | checklayers |
| **R4** | `unsafe` only in the trusted base. Every other crate root declares `#![forbid(unsafe_code)]`; `conf` and `mdi` declare `#![deny(unsafe_code)]` and opt in item by item (the entry macro; `unsafe trait` declarations). | checklayers + compiler |
| **R5** | `asm!`, `global_asm!`, `naked_asm!` only in `arch/*`. | checklayers |
| **R6** | No `static mut` anywhere. Shared state lives behind a lock, in an atomic, or in a `CpuLocal`. | checklayers |
| **R7** | Intra-kernel dependencies are declared in `[workspace.dependencies]` and used with `workspace = true`. External crates are forbidden unless added to `ALLOWED_EXTERNAL` in `tools/checklayers/main.rs`, with a reason. | checklayers |
| **R8** | Every `.rs` file starts with the MPL-2.0 header ([STYLE.md](STYLE.md)). | checklayers |
| **R9** | ABI personalities (`compat/*`) are leaves: only `conf` depends on them. The rest of the kernel never knows a userland ABI. | checklayers |
| **R10** | Every public API of the framework is sound: no sequence of safe calls from a service can cause undefined behaviour. | review |
| **R11** | Every `unsafe` block has a `// SAFETY:` comment, and every `unsafe fn` a `# Safety` section. | clippy (`undocumented_unsafe_blocks = "deny"`) + review |
| **R12** | MD code is reached through `mdi` traits; a missing implementation is a compile error. | compiler (`framework` asserts `Md: mdi::Machine`) |
| **R13** | No machine-dependent vocabulary in identifiers of MI code: architectures (`amd64`, `x86`, `riscv64`...), registers and tables (`rflags`, `cr3`, `msr`, `gdt`, `tss`, `ist`, `pml4`...), instructions and devices (`inb`, `hlt`, `tsc`, `apic`, `madt`, `i8254`...). The `mdi` contract speaks MI (`cpu_hwid`, "exception stacks"), and machine values come from the port (`KSTACK_PAGES`, `VM_NFREEORDER`, `AP_EXCEPTION_STACKS`). Only `sys/arch/*` and `arch/` directories are exempt. | checklayers (snake_case and CamelCase words of every identifier) |
| **R14** | The kernel compiles for every port, including the compile-only `sys/arch/riscv64`. If an MI change builds for amd64 only, MD code leaked into it. | `make check` (`cargo check --target` for each of `PORTS`) |
| **R15** | The kernel knows no ABI: no ABI vocabulary (`freebsd`, `linux`, `netbsd`, `darwin`, ...) in identifiers outside the personalities (`sys/compat/*`) and `sys/conf`. Everything ABI-specific (syscall numbers and register convention, errno and signal numbers, which signal a fault raises, the initial stack) sits behind `kern::kern_abi::SysEntVec`; the kernel offers ABI-neutral services (`kern_write`, `exit1`, `sigexit`, `kern_execve`) and names errors and signals without numbers (`sys::errno::Errno`, `sys::signal::Signal`, both exhaustive, so a new one does not compile until every ABI maps it). Running another ABI means writing `sys/compat/<abi>` and registering it in `conf`. | checklayers (words of every identifier) + compiler (exhaustive `match`) |

Source checks ignore comments and string literals, so documentation may
mention `target_arch`, `asm!` or `static mut` freely.

## Adding things

**A new service** (e.g. `sys/fs/tmpfs`): create the crate with
`#![forbid(unsafe_code)]`, add it to `members` and
`[workspace.dependencies]` in `sys/Cargo.toml`, and depend on `framework`
for anything hardware-related. If you need something the framework does
not offer, extend the framework (and keep R10), do not reach around it.

**A new MD function:** add it to the right `mdi` trait, implement it in
every `arch/*` (the compiler lists them), and wrap it in `framework` with
an MI, safe signature. If misuse could break memory safety, make the trait
an `unsafe trait` and write the contract down.

**A new architecture:** see "Porting to a new architecture" in
[ARCHITECTURE.md](ARCHITECTURE.md).

**An external crate:** avoid it. If it is worth it, add it to
`ALLOWED_EXTERNAL` with the reason, and remember that it becomes part of
the trusted base if any trusted crate uses it.

## Checking

```sh
make check          # runs tools/checklayers on sys/
```

Example output on a broken tree:

```
checklayers: sys/kern/Cargo.toml:16: R1: only framework (and other MD crates) may depend on MD crates (service `kern` -> md `amd64`)
checklayers: sys/kern/init_main.rs:90: R6: `static mut` is forbidden; use a lock, an atomic or CpuLocal
checklayers: 2 violation(s); see docs/LAYERING.md
```
