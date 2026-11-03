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

//! ELF definitions (FreeBSD's `sys/elf64.h`) and the kernel symbol table
//! the loader preloads (what FreeBSD's `db_fetch_ksymtab` reads).
//!
//! The loader copies the kernel's `.symtab` and `.strtab` after the image
//! and reports the area as `[MODINFOMD_SSYM, MODINFOMD_ESYM)`, laid out as
//!
//! ```text
//! u64 symtab size | symtab | u64 strtab size | strtab
//! ```

use crate::bytes::{read_u32, read_u64};

/// Size of an `Elf64_Sym`.
pub const ELF64_SYM_SIZE: usize = 24;
/// `STT_FUNC`: the symbol is a function.
pub const STT_FUNC: u8 = 2;

/// One `Elf64_Sym`.
#[derive(Clone, Copy, Debug)]
pub struct Sym {
    /// Offset of the name in the string table.
    pub name: u32,
    /// Type and binding (`ELF64_ST_TYPE`, `ELF64_ST_BIND`).
    pub info: u8,
    /// Value: the address, for functions and objects.
    pub value: u64,
    /// Size in bytes (0 if unknown).
    pub size: u64,
}

impl Sym {
    /// `ELF64_ST_TYPE(st_info)`.
    pub fn kind(&self) -> u8 {
        self.info & 0xf
    }
}

/// The preloaded kernel symbol table.
#[derive(Clone, Copy, Debug)]
pub struct KSymTab<'a> {
    symtab: &'a [u8],
    strtab: &'a [u8],
}

impl<'a> KSymTab<'a> {
    /// Parses the `[SSYM, ESYM)` area; `None` if the sizes do not fit in
    /// it (as `db_fetch_ksymtab` checks).
    pub fn new(area: &'a [u8]) -> Option<Self> {
        let symsz = usize::try_from(read_u64(area, 0)?).ok()?;
        let symtab = area.get(8..8usize.checked_add(symsz)?)?;
        let stroff = 8 + symsz;
        let strsz = usize::try_from(read_u64(area, stroff)?).ok()?;
        let strtab = area.get(stroff + 8..(stroff + 8).checked_add(strsz)?)?;
        Some(Self { symtab, strtab })
    }

    /// Every symbol.
    pub fn symbols(&self) -> impl Iterator<Item = Sym> + 'a {
        self.symtab
            .as_chunks::<ELF64_SYM_SIZE>()
            .0
            .iter()
            .filter_map(|e| {
                Some(Sym {
                    name: read_u32(e, 0)?,
                    info: *e.get(4)?,
                    value: read_u64(e, 8)?,
                    size: read_u64(e, 16)?,
                })
            })
    }

    /// The name of a symbol.
    pub fn name(&self, sym: &Sym) -> Option<&'a str> {
        let rest = self.strtab.get(sym.name as usize..)?;
        let end = rest.iter().position(|&b| b == 0)?;
        core::str::from_utf8(&rest[..end])
            .ok()
            .filter(|s| !s.is_empty())
    }

    /// The function containing `addr`, and the offset of `addr` in it
    /// (`linker_ddb_search_symbol`). Falls back to the closest function
    /// below `addr` when sizes are missing.
    pub fn lookup(&self, addr: u64) -> Option<(&'a str, u64)> {
        let mut best: Option<Sym> = None;
        for s in self
            .symbols()
            .filter(|s| s.kind() == STT_FUNC && s.value <= addr)
        {
            if s.size != 0 && addr < s.value + s.size {
                best = Some(s);
                break;
            }
            if s.size == 0 && best.is_none_or(|b| s.value > b.value) {
                best = Some(s);
            }
        }
        let s = best?;
        Some((self.name(&s)?, addr - s.value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sym(name: u32, info: u8, value: u64, size: u64) -> [u8; 24] {
        let mut e = [0u8; 24];
        e[0..4].copy_from_slice(&name.to_ne_bytes());
        e[4] = info;
        e[8..16].copy_from_slice(&value.to_ne_bytes());
        e[16..24].copy_from_slice(&size.to_ne_bytes());
        e
    }

    fn area() -> Vec<u8> {
        let strtab = b"\0btext\0hammer_time\0data\0";
        let mut symtab = Vec::new();
        symtab.extend(sym(0, 0, 0, 0)); // null symbol
        symtab.extend(sym(1, STT_FUNC, 0x1000, 0x20));
        symtab.extend(sym(7, STT_FUNC, 0x1100, 0x200));
        symtab.extend(sym(19, 1, 0x1150, 8)); // object inside a function
        let mut a = Vec::new();
        a.extend((symtab.len() as u64).to_ne_bytes());
        a.extend(&symtab);
        a.extend((strtab.len() as u64).to_ne_bytes());
        a.extend(strtab);
        a
    }

    #[test]
    fn finds_containing_function() {
        let a = area();
        let t = KSymTab::new(&a).unwrap();
        assert_eq!(t.lookup(0x1000), Some(("btext", 0)));
        assert_eq!(t.lookup(0x115d), Some(("hammer_time", 0x5d)));
        assert_eq!(t.lookup(0x10), None);
    }

    #[test]
    fn rejects_bad_sizes() {
        let mut a = area();
        assert!(KSymTab::new(&a[..a.len() - 1]).is_none());
        a[0] = 0xff;
        assert!(KSymTab::new(&a).is_none());
        assert!(KSymTab::new(&[]).is_none());
    }
}
