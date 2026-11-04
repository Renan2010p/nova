# Style

## Language

Everything in the Nova Project is written in **English**: code, comments,
documentation, commit messages.

## License header

Every Rust source file starts with this header (rule R8, checked by
`tools/checklayers`):

```rust
/*-
 * SPDX-License-Identifier: MPL-2.0
 *
 * Copyright (c) 2026 Renan Lucas Vieira Hilário
 *
 * This file is part of the Nova Project, the kernel of the
 * Renux "Nova" Project.
 *
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
```

Other files (`Cargo.toml`, `Makefile`, shell scripts, linker scripts)
start with at least the `SPDX-License-Identifier`, copyright and project
lines, in the comment syntax of the file.

If a file adapts code from FreeBSD or NetBSD, keep their copyright notice
and BSD license text below the MPL header (see [COPYRIGHT](../COPYRIGHT)).

## Names

* **Files and directories follow FreeBSD/NetBSD.** Before creating a
  file, look up where FreeBSD keeps the equivalent and use that path and
  name: `kern/kern_fork.rs`, `kern/subr_*.rs`, `vm/vm_map.rs`,
  `arch/amd64/machdep.rs`. Prefixes: `init_`, `kern_`, `subr_`, `sys_`,
  `vfs_`, `uipc_`, `sched_`, `vm_`.
* **Functions keep FreeBSD's names when they do the same job**
  (`mi_startup`, `hammer_time`, `cnputc`, `getmemsize`, `curcpu`), so the
  FreeBSD sources work as a reference. Rust conventions apply otherwise
  (`snake_case`, `CamelCase` types).
* **Constants that are part of an interface keep FreeBSD's exact names
  and values** (`MODINFO_*`, `RB_*`, `PSL_*`, `MSR_*`). Cite the FreeBSD
  header in the module doc.

## Messages

Nova must feel like FreeBSD to a Renux user, so:

* **When Nova does something FreeBSD reports, it prints FreeBSD's
  message**, with the same wording, spacing and number formats
  (`real memory  = 536870912 (512 MB)`, two spaces included). Copy the
  text from the FreeBSD sources (`/usr/src/sys/...`) and name the
  function it comes from in a comment or doc.
* **What FreeBSD only prints with `boot -v`, Nova only prints with
  `boot -v`** (`RB_VERBOSE`, `boot_verbose="YES"`).
* **Tunables keep FreeBSD's names and defaults** (`kern.hz`,
  `kern.panic_reboot_wait_time`, `boot_verbose`...).
* Nova-only messages say `Nova:` (or `selftest:`), so they are never
  mistaken for FreeBSD ones.
* Never print `FreeBSD` as the system's name: Nova is FreeBSD-compatible,
  not FreeBSD (trademark).

## Comments and documentation

* Every crate root has a `//!` block: what the crate is, its layer, and
  its FreeBSD counterpart.
* Every module has a `//!` line naming its FreeBSD counterpart, if any.
* Public items have `///` docs (`missing_docs` warns).
* Comments explain *why*. The code says *what*.
* Planned work is written as `TODO(topic): ...`, e.g. `TODO(smp)`.

## `unsafe`

* Only in the trusted base (`libkern`, `arch/*`, `framework`): rule R4.
* Every `unsafe` block has a `// SAFETY:` comment saying why each
  requirement holds *here* (enforced by clippy).
* Every `unsafe fn` has a `# Safety` section listing what the caller must
  guarantee. Inside it, unsafe operations still need their own `unsafe {}`
  block (`unsafe_op_in_unsafe_fn = "deny"`).
* Prefer a safe wrapper with a narrow contract over spreading `unsafe`
  around: `Ns8250::new` is `unsafe`, but `putc` is safe.
* Parse firmware and loader data with bounds-checked reads
  (`sys::bytes::read_u32`/`read_u64`) instead of casting pointers to
  structs.

## Explicit code

Nova's Rust is as explicit as Zig (rules R16 and R18 in
[LAYERING.md](LAYERING.md)):

* **Allocation is visible.** No `Box`/`Vec`/`format!`: `KBox::new(v,
  &M_PROC, M_WAITOK)?`. Prefer a fixed buffer when there is a bound
  (`[u8; MAXCOMLEN]`), and no allocation at all where one is not needed.
* **Conversions are named.** No `as`: `u64::from(x)`, `u32::try_from(x)`,
  or `libkern::conv` (`u64_from_usize`, `trunc_u8`, `fn_addr!`). Pointers:
  `.addr()`, `ptr::with_exposed_provenance`, `.cast()`.
* **Failure is handled or explained.** Errors are `Result`s
  (`KResult<T>`, `AllocError`). `unwrap()` is not allowed; `expect("why")`
  states the invariant that makes a value present; a deliberate panic says
  why in its message.
* **Context is explicit.** `M_WAITOK` only where sleeping is allowed (the
  check panics otherwise); spin locks and interrupt guards are values.

## Formatting and checks

```sh
cargo fmt                # in sys/ and tools/
make clippy              # must be clean
make check               # layering rules
make test                # host unit tests
```

The kernel is built with `overflow-checks = true` in release too: integer
overflow is a panic, not silent wrap-around. Use `wrapping_*`,
`checked_*` or `saturating_*` when wrap-around is intended.

## Commits

One logical change per commit, imperative subject line prefixed with the
area, the way FreeBSD does it:

```
amd64: set up the per-CPU area before the early console
kern: print the CPU count in mi_startup
```
