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


//! FreeBSD/amd64 signal frames (`sendsig` and `sys_sigreturn`,
//! `amd64/amd64/exec_machdep.c`; `sigcode`, `amd64/amd64/sigtramp.S`).
//!
//! The frame is FreeBSD's `struct sigframe`: the handler, a `ucontext_t`
//! (the mask to restore and the registers, with the `fxsave` area), and a
//! `siginfo_t`. Offsets from FreeBSD's headers (`tools/abidump` style).

use framework::user::{self, UserRegs, VmSpace};
use kern::kern_exec::vm_errno;
use libkern::conv::u64_from_usize;
use sys::errno::{Errno, KResult};
use sys::signal::{SigAction, SigSet};

use crate::arch::USRSTACK;
use crate::freebsd_signal::{SIGSET_SIZE, sigset_in, sigset_out};

/// The shared page (`SHAREDPAGE_LA48`), above the stack: the signal
/// trampoline lives there.
pub(crate) const SHAREDPAGE: u64 = USRSTACK;

/// `sigcode`: calls the handler (at the frame's start), then
/// `sigreturn(&frame.sf_uc)`:
///
/// ```text
/// call  *0(%rsp)
/// lea   16(%rsp), %rdi
/// pushq $0
/// mov   $SYS_sigreturn, %rax
/// syscall
/// 0: hlt
///    jmp 0b
/// ```
pub(crate) const SIGCODE: [u8; 23] = [
    0xff, 0x14, 0x24, // call *(%rsp)
    0x48, 0x8d, 0x7c, 0x24, 0x10, // lea 0x10(%rsp),%rdi
    0x6a, 0x00, // push $0
    0x48, 0xc7, 0xc0, 0xa1, 0x01, 0x00, 0x00, // mov $417,%rax
    0x0f, 0x05, // syscall
    0xf4, // hlt
    0xeb, 0xfd, // jmp hlt
    0x90,
];

/// `sizeof(struct sigframe)` and its parts.
const SIGFRAME_SIZE: usize = 976;
const SF_UC: usize = 16;
const SF_SI: usize = 896;
/// `sizeof(ucontext_t)`.
const UCONTEXT_SIZE: usize = 880;
const UC_MCONTEXT: usize = 16;
const UC_STACK_FLAGS: usize = 824 + 16;
/// `sizeof(mcontext_t)`: its `mc_len`.
const MCONTEXT_SIZE: u64 = 800;
/// `mcontext_t` fields: the general registers from `mc_rdi` on, in this
/// order (`struct trapframe`'s).
const MC_REGS: usize = 8;
const MC_TRAPNO: usize = 128;
const MC_ADDR: usize = 136;
const MC_FLAGS: usize = 144;
const MC_ERR: usize = 152;
const MC_RIP: usize = 160;
const MC_CS: usize = 168;
const MC_RFLAGS: usize = 176;
const MC_RSP: usize = 184;
const MC_SS: usize = 192;
const MC_LEN: usize = 200;
const MC_FPFORMAT: usize = 208;
const MC_OWNEDFP: usize = 216;
const MC_FPSTATE: usize = 224;
const MC_FSBASE: usize = 736;

const MC_HASSEGS: u32 = 0x1;
const MC_HASBASES: u32 = 0x2;
const MC_FPFMT_XMM: u64 = 0x1_0002;
const MC_FPOWNED_FPU: u64 = 0x2_0001;
const SS_DISABLE: u32 = 4;
/// `SI_KERNEL`.
const SI_KERNEL: i32 = 0x1_0006;

/// User code and stack selectors (`_ucodesel`, `_udatasel`).
const UCODESEL: u64 = 0x43;
const UDATASEL: u64 = 0x3b;

/// Flags a program may change (`PSL_USERCHANGE`): C, PF, AF, ZF, SF, TF,
/// DF, OF, NT, RF, AC, ID.
const PSL_USERCHANGE: u64 = 0x25_4dd5;
const PSL_T: u64 = 0x100;
const PSL_D: u64 = 0x400;

/// The registers of `struct trapframe` from `tf_rdi`, as `mcontext_t`
/// keeps them.
fn gprs(r: &mut UserRegs) -> [&mut u64; 15] {
    [
        &mut r.rdi, &mut r.rsi, &mut r.rdx, &mut r.rcx, &mut r.r8, &mut r.r9, &mut r.rax, &mut r.rbx, &mut r.rbp,
        &mut r.r10, &mut r.r11, &mut r.r12, &mut r.r13, &mut r.r14, &mut r.r15,
    ]
}

fn put(b: &mut [u8], off: usize, v: &[u8]) {
    if let Some(dst) = b.get_mut(off..off + v.len()) {
        dst.copy_from_slice(v);
    }
}

fn get8(b: &[u8], off: usize) -> u64 {
    b.get(off..off + 8).and_then(|s| s.try_into().ok()).map_or(0, u64::from_le_bytes)
}

/// `PSL_C`, the carry flag: set, a system call failed.
const PSL_C: u64 = 0x1;

/// A `ucontext_t` for `regs` and `mask` into `uc` (`get_mcontext`):
/// with `clear_ret`, as a system call that returns 0 leaves them
/// (`GET_MC_CLEAR_RET`, for `getcontext`).
fn ucontext_out(regs: &UserRegs, mask: SigSet, clear_ret: bool, uc: &mut [u8]) {
    let mc = UC_MCONTEXT;
    put(uc, 0, &sigset_out(mask));
    put(uc, UC_STACK_FLAGS, &SS_DISABLE.to_le_bytes());
    let mut saved = *regs;
    if clear_ret {
        saved.rax = 0;
        saved.rdx = 0;
        saved.rflags &= !PSL_C;
    }
    for (i, r) in gprs(&mut saved).into_iter().enumerate() {
        put(uc, mc + MC_REGS + 8 * i, &r.to_le_bytes());
    }
    put(uc, mc + MC_TRAPNO, &u32::try_from(regs.trapno).unwrap_or(0).to_le_bytes());
    put(uc, mc + MC_ADDR, &regs.fault_addr.to_le_bytes());
    put(uc, mc + MC_FLAGS, &(MC_HASSEGS | MC_HASBASES).to_le_bytes());
    put(uc, mc + MC_ERR, &regs.err.to_le_bytes());
    put(uc, mc + MC_RIP, &regs.rip.to_le_bytes());
    put(uc, mc + MC_CS, &UCODESEL.to_le_bytes());
    put(uc, mc + MC_RFLAGS, &saved.rflags.to_le_bytes());
    put(uc, mc + MC_RSP, &regs.rsp.to_le_bytes());
    put(uc, mc + MC_SS, &UDATASEL.to_le_bytes());
    put(uc, mc + MC_LEN, &MCONTEXT_SIZE.to_le_bytes());
    put(uc, mc + MC_FPFORMAT, &MC_FPFMT_XMM.to_le_bytes());
    put(uc, mc + MC_OWNEDFP, &MC_FPOWNED_FPU.to_le_bytes());
    if let Some(fp) = uc.get_mut(mc + MC_FPSTATE..mc + MC_FPSTATE + 512) {
        user::fpu_save(fp);
    }
    put(uc, mc + MC_FSBASE, &regs.fsbase.to_le_bytes());
}

/// Saves the caller's context at `ucp` (`sys_getcontext`): it resumes
/// there, `getcontext` returning 0, when `setcontext` loads it.
pub(crate) fn getcontext(vm: &VmSpace, regs: &UserRegs, mask: SigSet, ucp: u64) -> KResult<()> {
    let mut uc = [0u8; UCONTEXT_SIZE];
    ucontext_out(regs, mask, true, &mut uc);
    vm.copyout(ucp, &uc).map_err(vm_errno)
}

/// Maps the shared page with the signal trampoline in a new image.
pub(crate) fn map_sigcode(vm: &VmSpace) -> KResult<()> {
    let prot = framework::user::Prot { read: true, write: false, exec: true };
    vm.map_zeroed(SHAREDPAGE, 4096, prot).map_err(vm_errno)?;
    vm.copyout_init(SHAREDPAGE, &SIGCODE).map_err(vm_errno)
}

/// Builds the frame for `signo`'s handler on the user stack and points
/// `regs` at it (`sendsig`).
pub(crate) fn sendsig(
    vm: &VmSpace,
    signo: i32,
    handler: u64,
    action: &SigAction,
    mask: SigSet,
    regs: &mut UserRegs,
) -> KResult<()> {
    let mut sf = [0u8; SIGFRAME_SIZE];
    if let Some(uc) = sf.get_mut(SF_UC..SF_UC + UCONTEXT_SIZE) {
        ucontext_out(regs, mask, false, uc);
    }

    // Below the red zone, 16-aligned.
    let sfp = (regs.rsp.wrapping_sub(128).wrapping_sub(u64_from_usize(SIGFRAME_SIZE))) & !0xf;
    put(&mut sf, 0, &handler.to_le_bytes());
    if action.flags.siginfo {
        put(&mut sf, SF_SI, &signo.to_le_bytes());
        put(&mut sf, SF_SI + 8, &SI_KERNEL.to_le_bytes());
        regs.rsi = sfp + u64_from_usize(SF_SI);
    } else {
        regs.rsi = 0;
    }
    vm.copyout(sfp, &sf).map_err(vm_errno)?;
    // The handler starts with a clean FPU (`fpstate_drop`).
    user::fpu_load(None);
    regs.rdi = u64::from(signo.unsigned_abs());
    regs.rdx = sfp + u64_from_usize(SF_UC);
    regs.rcx = 0;
    regs.rsp = sfp;
    regs.rip = SHAREDPAGE;
    regs.rflags &= !(PSL_T | PSL_D);
    // With a registered TLS base, the handler runs with it (`PCB_TLSBASE`).
    if regs.tlsbase_set {
        regs.fsbase = regs.tlsbase;
    }
    Ok(())
}

/// Takes the frame at `ucp` down (`sys_sigreturn`, and `setcontext`): the registers and
/// floating-point state come back; returns the mask to restore. `EINVAL`
/// for a context a program may not set (privileged flags, another code
/// segment).
pub(crate) fn sigreturn(vm: &VmSpace, ucp: u64, regs: &mut UserRegs) -> KResult<SigSet> {
    let mut uc = [0u8; UCONTEXT_SIZE];
    vm.copyin(ucp, &mut uc).map_err(vm_errno)?;
    let mc = UC_MCONTEXT;
    if get8(&uc, mc + MC_LEN) != MCONTEXT_SIZE {
        return Err(Errno::EINVAL);
    }
    let rflags = get8(&uc, mc + MC_RFLAGS);
    if (rflags ^ regs.rflags) & !PSL_USERCHANGE != 0 || get8(&uc, mc + MC_CS) != UCODESEL {
        return Err(Errno::EINVAL);
    }
    let mut new = *regs;
    for (i, r) in gprs(&mut new).into_iter().enumerate() {
        *r = get8(&uc, mc + MC_REGS + 8 * i);
    }
    new.rip = get8(&uc, mc + MC_RIP);
    new.rsp = get8(&uc, mc + MC_RSP);
    new.rflags = rflags;
    new.fsbase = get8(&uc, mc + MC_FSBASE);
    let fp = uc.get(mc + MC_FPSTATE..mc + MC_FPSTATE + 512);
    user::fpu_load(if get8(&uc, mc + MC_OWNEDFP) != 0 { fp } else { None });
    *regs = new;
    let mut m = [0u8; SIGSET_SIZE];
    m.copy_from_slice(uc.get(..SIGSET_SIZE).ok_or(Errno::EFAULT)?);
    Ok(sigset_in(&m))
}

/// Makes the program issue the same system call again (`ERESTART`): back
/// over the 2-byte `syscall` instruction; `%rax` and the arguments are
/// still as they were.
pub(crate) fn cpu_syscall_restart(regs: &mut UserRegs) {
    regs.rip -= 2;
}
