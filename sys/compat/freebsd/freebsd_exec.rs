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

use alloc::vec::Vec;

use framework::user::VmSpace;
use kern::kern_abi::ImageParams;
use kern::kern_exec::{SGROWSIZ_INITIAL, vm_errno};
use sys::errno::{Errno, KResult};

use crate::FREEBSD_VERSION;
use crate::arch::{PAGESIZES, USRSTACK};

const AT_NULL: u64 = 0;
const AT_PHDR: u64 = 3;
const AT_PHENT: u64 = 4;
const AT_PHNUM: u64 = 5;
const AT_PAGESZ: u64 = 6;
const AT_FLAGS: u64 = 8;
const AT_ENTRY: u64 = 9;
const AT_BASE: u64 = 7;
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

    let mut path = Vec::from(img.execpath.as_bytes());
    path.push(0);
    destp = rounddown(destp - path.len() as u64, PTR);
    let execpathp = destp;
    put(execpathp, &path)?;

    // TODO(random): `arc4rand`; there is no entropy source yet.
    destp -= CANARY_LEN;
    let canary = destp;
    put(canary, &[0; CANARY_LEN as usize])?;

    let pagesizeslen = PAGESIZES.len() as u64 * PTR;
    destp = rounddown(destp - pagesizeslen, PTR);
    let pagesizes = destp;
    for (i, &ps) in PAGESIZES.iter().enumerate() {
        word(pagesizes + i as u64 * PTR, ps)?;
    }

    let mut strings = Vec::new();
    for s in img.argv.iter().chain(img.envv) {
        strings.extend_from_slice(s.as_bytes());
        strings.push(0);
    }
    // Everything must fit in the mapped stack, with room to run.
    let vectors = (img.argv.len() + img.envv.len() + 2) as u64 * PTR + AT_COUNT * 16;
    if strings.len() as u64 + vectors > ARG_MAX {
        return Err(Errno::E2BIG);
    }
    destp = rounddown(destp - strings.len() as u64, PTR);
    let ustringp = destp;
    put(ustringp, &strings)?;

    destp = rounddown(destp - AT_COUNT * 16, PTR);
    let (argc, envc) = (img.argv.len() as u64, img.envv.len() as u64);
    let argv = destp - (argc + 1 + envc + 1) * PTR;
    let envv = argv + (argc + 1) * PTR;
    let mut s = ustringp;
    let mut vectp = argv;
    for (i, a) in img.argv.iter().chain(img.envv).enumerate() {
        word(vectp, s)?;
        vectp += PTR;
        s += a.len() as u64 + 1;
        if i as u64 + 1 == argc {
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
    put(ps_strings + 8, &(argc as u32).to_le_bytes())?;
    word(ps_strings + 16, envv)?;
    put(ps_strings + 24, &(envc as u32).to_le_bytes())?;

    let aux = [
        (AT_PHDR, img.phdr),
        (AT_PHENT, u64::from(img.phent)),
        (AT_PHNUM, u64::from(img.phnum)),
        (AT_PAGESZ, framework::PAGE_SIZE as u64),
        (AT_FLAGS, 0),
        (AT_ENTRY, img.entry),
        (AT_BASE, 0),
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
    assert!(aux.len() as u64 <= AT_COUNT, "auxv: more entries than AT_COUNT");
    for (i, (k, v)) in aux.iter().enumerate() {
        word(auxv + i as u64 * 16, *k)?;
        word(auxv + i as u64 * 16 + 8, *v)?;
    }

    // freebsd_fixup: argc just below argv.
    let stack = argv - PTR;
    word(stack, argc)?;
    Ok(stack)
}
