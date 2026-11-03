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
| **R3** | `cfg(target_arch …)` appears only in `arch/*`, in `framework`, or in files under an `arch/` directory of a service (e.g. `compat/freebsd/arch/amd64.rs`). | checklayers |
| **R4** | `unsafe` only in the trusted base. Every other crate root declares `#![forbid(unsafe_code)]`; `conf` and `mdi` declare `#![deny(unsafe_code)]` and opt in item by item (the entry macro; `unsafe trait` declarations). | checklayers + compiler |
| **R5** | `asm!`, `global_asm!`, `naked_asm!` only in `arch/*`. | checklayers |
| **R6** | No `static mut` anywhere. Shared state lives behind a lock, in an atomic, or in a `CpuLocal`. | checklayers |
| **R7** | Intra-kernel dependencies are declared in `[workspace.dependencies]` and used with `workspace = true`. External crates are forbidden unless added to `ALLOWED_EXTERNAL` in `tools/checklayers/main.rs`, with a reason. | checklayers |
| **R8** | Every `.rs` file starts with the MPL-2.0 header ([STYLE.md](STYLE.md)). | checklayers |
| **R9** | ABI personalities (`compat/*`) are leaves: only `conf` depends on them. The rest of the kernel never knows a userland ABI. | checklayers |
| **R10** | Every public API of the framework is sound: no sequence of safe calls from a service can cause undefined behaviour. | review |
| **R11** | Every `unsafe` block has a `// SAFETY:` comment, and every `unsafe fn` a `# Safety` section. | clippy (`undocumented_unsafe_blocks = "deny"`) + review |
| **R12** | MD code is reached through `mdi` traits; a missing implementation is a compile error. | compiler (`framework` asserts `md::Machine: mdi::Machine`) |

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
