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
| No undefined behaviour in those tests | Miri (`make miri`) |
| Allocator, range checks, ELF parsing and user flags are correct for every input | Kani (`make kani`), see below |

## Phase 1: language-level guarantees (milestone 4b, done)

**Miri** (`make miri`) runs the host unit tests under an interpreter that
detects undefined behaviour: invalid pointers, misalignment, data races,
uninitialized reads. It cannot run the kernel itself (no hardware, no
`asm!`), but it runs the crates the trusted base is built from: the buddy
allocator, the ELF and boot-metadata parsers, the demangler. All 37
tests of `libkern`, `sys` and `x86` pass with no undefined behaviour.

**Kani** (`make kani`) proves, by bounded model checking, that a function
cannot panic, overflow or break an assertion for *every* input (up to the
stated bounds). The kernel builds with overflow checks on, so for the
parsers "cannot panic" means "a crafted executable cannot panic the
kernel".

| Proof | What holds, for every input |
|---|---|
| `libkern::buddy` `one_block_any_range` | for any zone range and order, a block is aligned and inside the zone, no page is lost, and freeing it coalesces back to exactly the free lists `add_range` built |
| `libkern::buddy` `two_blocks_are_disjoint` | any two requests get disjoint blocks, every page is accounted for, and freeing both coalesces back |
| `libkern::range` `range_end_is_exact` | an accepted user range does not wrap and lies below the limit; every rejected one would |
| `libkern::range` `page_range_end_is_aligned` | an accepted page range is page aligned at both ends (any power-of-two page) |
| `sys::elf` `ehdr_parse_never_panics` | any bytes, any length: no panic, no truncated header accepted |
| `sys::elf` `phdrs_never_panic` | any offsets, sizes and counts in the header: no panic, no out-of-bounds read |
| `sys::elf` `segment_notes_never_panic` | any `PT_NOTE` contents: no panic or overflow, every note lies inside its segment |
| `x86::psl` `user_rflags_is_safe` | whatever flags a program asks for, user mode runs with interrupts on, IOPL 0, no nested-task or virtual-8086 mode |
| `x86::psl` `user_rflags_keeps_user_bits` | the flags a program may change are kept |

`VmSpace` uses the proven range checks and `cpu_user_enter` the proven
flag sanitization, so the proofs cover the code the kernel runs, not a
copy. Zones and buffers are small (a 4-page buddy zone, 40- to 80-byte
images) because the checker's memory grows fast with them; each proof
runs alone, limited to `KANI_MEM` (2.5 GB by default), and the whole set
takes about a minute.

Neither tool builds the kernel, which stays on the pinned stable Rust:
Miri ships with nightly Rust, and Kani brings its own toolchain.

Next proofs: the free-list invariants of `buddy` (`free_blocks` agrees
with `state`), `kenv` and preload metadata parsing, and the user range
loop of `VmSpace::each_page` once it moves into `libkern`.

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
