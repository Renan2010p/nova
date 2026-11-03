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

//! The kernel linker (FreeBSD's `kern/kern_linker.c`): for now, the
//! kernel's own symbol table, which the loader preloaded, used to name the
//! functions in backtraces (`linker_ddb_search_symbol`).
//!
//! TODO(kld): loadable kernel modules.

use core::fmt::{self, Write};

use framework::BootInfo;
use framework::sync::OnceLock;
use sys::elf::KSymTab;

static KSYMS: OnceLock<KSymTab<'static>> = OnceLock::new();

/// Takes the preloaded symbol table (`linker_preload`).
pub fn linker_init(boot: &BootInfo<'_>) {
    if let Some(table) = KSymTab::new(boot.ksyms) {
        let _ = KSYMS.set(table);
    }
}

/// Writes `addr` as `function+0xoffset`, with Rust names demangled, or
/// as `??` if unknown. Lock-free, so it can be used from `panic`.
pub fn print_symbol(w: &mut dyn Write, addr: usize) -> fmt::Result {
    // A return address may point just past the end of a call at the end
    // of a function, so look up the byte before it.
    let found = KSYMS
        .get()
        .and_then(|t| t.lookup((addr as u64).wrapping_sub(1)));
    let Some((name, off)) = found else {
        return w.write_str("??");
    };
    if !libkern::rust_demangle::demangle(name, &mut WriteRef(w)) {
        w.write_str(name)?;
    }
    write!(w, "+{:#x}", off + 1)
}

/// Adapts `&mut dyn Write` for APIs taking `impl Write`.
struct WriteRef<'a>(&'a mut dyn Write);

impl Write for WriteRef<'_> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.0.write_str(s)
    }
}
