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

//! The initial stack of a FreeBSD process: FreeBSD's
//! `exec_copyout_strings` (`kern/kern_exec.c`), `freebsd_copyout_auxargs`
//! and `freebsd_fixup` (`kern/imgact_elf.c`), with the same layout, from
//! the top of the stack down:
//!
//! ```text
//! usrstack -> +---------------------------+
//!             | struct ps_strings         |
//!             | execpath (AT_EXECPATH)    |
//!             | canary (AT_CANARY)        |
//!             | pagesizes (AT_PAGESIZES)  |
//!             | argv and envv strings     |
//!             | room for AT_COUNT auxv    |  <- auxv starts after envp's NULL
//!             | envp[], NULL              |
//!             | argv[], NULL              |
//! stack ->    | argc                      |
//!             +---------------------------+
//! ```

use libkern::conv::{u64_from_usize, usize_from_u64};
use framework::user::VmSpace;
use kern::kern_abi::ImageParams;
use kern::kern_exec::{SGROWSIZ_INITIAL, vm_errno};
use sys::elf::{AT_BASE, AT_ENTRY, AT_PHDR, AT_PHENT, AT_PHNUM};
use sys::errno::{Errno, KResult};

use crate::FREEBSD_VERSION;
use crate::arch::{PAGESIZES, USRSTACK};

const AT_NULL: u64 = 0;
const AT_PAGESZ: u64 = 6;
const AT_FLAGS: u64 = 8;
const AT_EHDRFLAGS: u64 = 24;
const AT_EXECPATH: u64 = 15;
const AT_OSRELDATE: u64 = 18;
const AT_CANARY: u64 = 16;
const AT_CANARYLEN: u64 = 17;
const AT_NCPUS: u64 = 19;
const AT_PAGESIZES: u64 = 20;
const AT_PAGESIZESLEN: u64 = 21;
const AT_STACKPROT: u64 = 23;
const AT_BSDFLAGS: u64 = 27;
const AT_ARGC: u64 = 28;
const AT_ARGV: u64 = 29;
const AT_ENVC: u64 = 30;
const AT_ENVV: u64 = 31;
const AT_PS_STRINGS: u64 = 32;
const AT_USRSTACKBASE: u64 = 35;
const AT_USRSTACKLIM: u64 = 36;
/// Count of defined auxv types: the room reserved for the vector.
const AT_COUNT: u64 = 40;

/// `sizeof(struct ps_strings)` on LP64.
const PS_STRINGS_SIZE: u64 = 32;
/// `sizeof(long) * 8`.
const CANARY_LEN: u64 = 64;
/// `VM_PROT_READ | VM_PROT_WRITE`: the stack is not executable.
const VM_PROT_RW: u64 = 3;
/// Most bytes of arguments and environment (FreeBSD's `ARG_MAX` is
/// larger; Nova's stack is smaller for now).
const ARG_MAX: u64 = SGROWSIZ_INITIAL / 4;
/// `sizeof(void *)`.
const PTR: u64 = 8;

const fn rounddown(v: u64, a: u64) -> u64 {
    v & !(a - 1)
}

/// Lays out the stack in `vm` and returns the stack base (the address of
/// `argc`).
pub(crate) fn copyout_strings(vm: &VmSpace, img: &ImageParams<'_>) -> KResult<u64> {
    let put = |va: u64, b: &[u8]| vm.copyout_init(va, b).map_err(vm_errno);
    let word = |va: u64, w: u64| put(va, &w.to_le_bytes());

    let ps_strings = USRSTACK - PS_STRINGS_SIZE;
    let mut destp = ps_strings;

    // The path, NUL-terminated, written in two pieces: nothing is
    // assembled in kernel memory.
    let path_len = u64_from_usize(img.execpath.len()) + 1;
    destp = rounddown(destp - path_len, PTR);
    let execpathp = destp;
    put(execpathp, img.execpath.as_bytes())?;
    put(execpathp + path_len - 1, &[0])?;

    // The stack protector's canary (`arc4rand`).
    let mut bytes = [0u8; usize_from_u64(CANARY_LEN)];
    kern::kern_random::arc4random_buf(&mut bytes);
    destp -= CANARY_LEN;
    let canary = destp;
    put(canary, &bytes)?;

    let pagesizeslen = u64_from_usize(PAGESIZES.len()) * PTR;
    destp = rounddown(destp - pagesizeslen, PTR);
    let pagesizes = destp;
    for (i, &ps) in PAGESIZES.iter().enumerate() {
        word(pagesizes + u64_from_usize(i) * PTR, ps)?;
    }

    // argv then envv strings, each NUL-terminated, written in place.
    let strings_len: u64 = img.argv.iter().chain(img.envv).map(|s| u64_from_usize(s.len()) + 1).sum();
    // Everything must fit in the mapped stack, with room to run.
    let vectors = u64_from_usize(img.argv.len() + img.envv.len() + 2) * PTR + AT_COUNT * 16;
    if strings_len + vectors > ARG_MAX {
        return Err(Errno::E2BIG);
    }
    destp = rounddown(destp - strings_len, PTR);
    let ustringp = destp;
    let mut at = ustringp;
    for s in img.argv.iter().chain(img.envv) {
        put(at, s.as_bytes())?;
        put(at + u64_from_usize(s.len()), &[0])?;
        at += u64_from_usize(s.len()) + 1;
    }

    destp = rounddown(destp - AT_COUNT * 16, PTR);
    let (argc, envc) = (u64_from_usize(img.argv.len()), u64_from_usize(img.envv.len()));
    let argv = destp - (argc + 1 + envc + 1) * PTR;
    let envv = argv + (argc + 1) * PTR;
    let mut s = ustringp;
    let mut vectp = argv;
    for (i, a) in img.argv.iter().chain(img.envv).enumerate() {
        word(vectp, s)?;
        vectp += PTR;
        s += u64_from_usize(a.len()) + 1;
        if u64_from_usize(i) + 1 == argc {
            word(vectp, 0)?; // the NULL between argv and envp
            vectp += PTR;
        }
    }
    if argc == 0 {
        word(vectp, 0)?;
        vectp += PTR;
    }
    word(vectp, 0)?; // end of envp
    let auxv = vectp + PTR;

    // struct ps_strings
    word(ps_strings, argv)?;
    put(ps_strings + 8, &u32::try_from(argc).map_err(|_| Errno::E2BIG)?.to_le_bytes())?;
    word(ps_strings + 16, envv)?;
    put(ps_strings + 24, &u32::try_from(envc).map_err(|_| Errno::E2BIG)?.to_le_bytes())?;

    // What the image format contributed (the program headers, for ELF).
    // The core does not know these tags; the ABI does.
    let aux_value = |tag: u64| img.aux.iter().find(|e| e.tag == tag).map_or(0, |e| e.value);
    let aux = [
        (AT_PHDR, aux_value(AT_PHDR)),
        (AT_PHENT, aux_value(AT_PHENT)),
        (AT_PHNUM, aux_value(AT_PHNUM)),
        (AT_PAGESZ, u64_from_usize(framework::PAGE_SIZE)),
        (AT_FLAGS, 0),
        (AT_ENTRY, img.aux.iter().find(|e| e.tag == AT_ENTRY).map_or(img.entry, |e| e.value)),
        (AT_BASE, aux_value(AT_BASE)),
        (AT_EHDRFLAGS, 0),
        (AT_EXECPATH, execpathp),
        (AT_OSRELDATE, u64::from(FREEBSD_VERSION)),
        (AT_CANARY, canary),
        (AT_CANARYLEN, CANARY_LEN),
        (AT_NCPUS, u64::from(framework::cpu::mp_ncpus())),
        (AT_PAGESIZES, pagesizes),
        (AT_PAGESIZESLEN, pagesizeslen),
        (AT_STACKPROT, VM_PROT_RW),
        (AT_BSDFLAGS, 0),
        (AT_ARGC, argc),
        (AT_ARGV, argv),
        (AT_ENVC, envc),
        (AT_ENVV, envv),
        (AT_PS_STRINGS, ps_strings),
        (AT_USRSTACKBASE, USRSTACK),
        (AT_USRSTACKLIM, SGROWSIZ_INITIAL),
        (AT_NULL, 0),
    ];
    assert!(u64_from_usize(aux.len()) <= AT_COUNT, "auxv: more entries than AT_COUNT");
    for (i, (k, v)) in aux.iter().enumerate() {
        word(auxv + u64_from_usize(i) * 16, *k)?;
        word(auxv + u64_from_usize(i) * 16 + 8, *v)?;
    }

    // freebsd_fixup: argc just below argv.
    let stack = argv - PTR;
    word(stack, argc)?;
    Ok(stack)
}
