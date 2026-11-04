# Verification

Memory safety is not functional correctness. A kernel can be 100% safe
Rust and still map the wrong page or deadlock its scheduler. Nova is
verified in two phases, aimed where the framekernel concentrates the
risk: the trusted base (`sys/framework`, `sys/arch/*`, `sys/libkern`).
The services (`kern`, `vm`, `compat/*`, later `dev`) are
`#![forbid(unsafe_code)]` and inherit memory safety from the type system,
provided the trusted base's API is sound.

## What holds today

| Property | How |
|---|---|
| Services contain no `unsafe` | `#![forbid(unsafe_code)]`, enforced by `checklayers` (R4) |
| Every `unsafe` block states why it is sound | clippy `undocumented_unsafe_blocks = "deny"` (R11) |
| MI code has no machine-dependent knowledge | `checklayers` R13, and the kernel builds for riscv64 (R14) |
| The kernel knows no ABI | `checklayers` R15; exhaustive `Errno`/`Signal` force every ABI to map every value |
| The FreeBSD syscall table is FreeBSD's | generated from `syscalls.master`; output identical to FreeBSD's `syscalls.c` and `syscall.h` |
| The kernel never holds references into user memory | `VmSpace` only copies (`copyin`/`copyout`) |
| Pure logic is tested | host unit tests (`make test`) of `libkern`, `sys`, `x86` |

## Phase 1: language-level guarantees (milestone 4b)

* **Miri** runs the host unit tests under an interpreter that detects
  undefined behaviour (invalid pointers, misalignment, data races, reads
  of uninitialized memory). It cannot run the kernel itself (no hardware,
  no `asm!`), but the crates it can run are the ones the trusted base is
  built from: the buddy allocator, the ELF and boot-metadata parsers, the
  demangler. Target: `make miri`.
* **Kani** (bounded model checking) proves that a function cannot panic
  or break an assertion for *every* input up to a bound. First proofs:
  * `libkern::buddy`: blocks handed out never overlap, a freed block is
    never handed out twice, coalescing preserves the free set;
  * `sys::elf`: `Ehdr::parse`, `phdrs` and `notes` never read out of
    bounds or panic, whatever the bytes;
  * `framework::user::VmSpace`'s range checks: no overflow, nothing at or
    above `VM_MAXUSER_ADDRESS` accepted;
  * the amd64 `cpu_user_enter` checks: user mode is never entered with
    IOPL ≠ 0, interrupts off, or a kernel `%rip`/`%rsp`.

  Target: `make kani`.

Both need tools outside the pinned toolchain (Miri ships only with
nightly Rust; Kani is installed with `cargo install kani-verifier`). They
verify the code; they never build the kernel, which stays on the pinned
stable Rust.

## Phase 2: functional correctness

Prove that the trusted base meets a written specification, starting with
the invariants everything else relies on:

| Component | Specification |
|---|---|
| `libkern::buddy`, `framework::mm` | every physical page has at most one owner |
| `pmap` | `pmap_enter_user` changes only the user half; the kernel half of every address space equals the kernel's |
| `framework::task` | every switch happens with exactly one interrupt guard held; a task never runs on two CPUs at once |
| `framework::user` | the kernel reaches user memory only by copying; a space is freed only when no CPU uses it |

Tools: **Verus** (SMT-based deductive verification of Rust, with
`requires`/`ensures`, loop invariants and ghost code) for these
invariants, starting with a pilot on `libkern::buddy`; **Aeneas** or
**Hax** (Rust to Lean 4 / Coq) for deep proofs of single algorithms, if
one ever needs more than Verus can do. Verus has its own toolchain and
syntax, so verified modules must be kept identical to the code that is
built.

Assumed, as by every verified kernel (seL4 included): the hardware, the
compiler, and the hand-written assembly (`locore`, `exception`,
`usermode`, `swtch`, `mpboot`), which are kept small and reviewed.

## FreeBSD compatibility

There is no formal specification of the FreeBSD ABI: FreeBSD is the
specification. Compatibility is therefore tested, not proven, by
differential testing: FreeBSD's own test suite (`/usr/tests`, kyua) and
syscall fuzzing (syzkaller supports FreeBSD) on both kernels, comparing
results. Where FreeBSD publishes data, Nova derives from it instead of
copying by hand (`make sysent`, and `tools/abidump` for struct layouts).
