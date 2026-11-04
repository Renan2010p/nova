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
//! functions in backtraces (`linker_ddb_search_symbol`), and the files the
//! loader preloaded (`preload_search_by_name`).
//!
//! TODO(kld): loadable kernel modules (milestone 7b), which needs the
//! kernel linked with `.dynamic` and `DT_HASH`, as the loader looks
//! symbols up there.

use libkern::conv::u64_from_usize;
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

/// The contents of the file the loader preloaded as `name`
/// (`preload_search_by_name` and `preload_fetch_addr`), if any.
pub fn preload_file(boot: &BootInfo<'_>, name: &str) -> Option<&'static [u8]> {
    let m = boot.preload.modules().find(|m| m.name() == Some(name))?;
    let off = usize::try_from(m.addr()?.checked_sub(boot.loaded_base)?).ok()?;
    let size = usize::try_from(m.size()?).ok()?;
    boot.loaded.get(off..off.checked_add(size)?)
}

/// Writes `addr` as `function+0xoffset`, with Rust names demangled, or
/// as `??` if unknown. Lock-free, so it can be used from `panic`.
///
/// A return address may point just past a call that ends its function,
/// so for those (`is_return`) the byte before is looked up, as FreeBSD's
/// `db_nextframe` does.
pub fn print_symbol(w: &mut dyn Write, addr: usize, is_return: bool) -> fmt::Result {
    let back = u64::from(is_return);
    let found = KSYMS
        .get()
        .and_then(|t| t.lookup(u64_from_usize(addr).wrapping_sub(back)));
    let Some((name, off)) = found else {
        return w.write_str("??");
    };
    if !libkern::rust_demangle::demangle(name, &mut WriteRef(w)) {
        w.write_str(name)?;
    }
    write!(w, "+{:#x}", off + back)
}

/// Adapts `&mut dyn Write` for APIs taking `impl Write`.
struct WriteRef<'a>(&'a mut dyn Write);

impl Write for WriteRef<'_> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.0.write_str(s)
    }
}
