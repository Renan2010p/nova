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

//! ELF definitions (FreeBSD's `sys/elf64.h`, `sys/elf_common.h`): the
//! generic parts of an executable's headers, which `kern` reads without
//! knowing any ABI, and the kernel symbol table the loader preloads (what
//! FreeBSD's `db_fetch_ksymtab` reads).
//!
//! The loader copies the kernel's `.symtab` and `.strtab` after the image
//! and reports the area as `[MODINFOMD_SSYM, MODINFOMD_ESYM)`, laid out as
//!
//! ```text
//! u64 symtab size | symtab | u64 strtab size | strtab
//! ```

use crate::bytes::{read_u32, read_u64};

/// `\x7fELF`.
pub const ELFMAG: &[u8; 4] = b"\x7fELF";
/// `EI_CLASS` value of 64-bit files.
pub const ELFCLASS64: u8 = 2;
/// `EI_DATA` value of little-endian files.
pub const ELFDATA2LSB: u8 = 1;
/// `e_type` of executables.
pub const ET_EXEC: u16 = 2;
/// `e_type` of position-independent executables and shared objects.
pub const ET_DYN: u16 = 3;
/// `p_type` of a loadable segment.
pub const PT_LOAD: u32 = 1;
/// `p_type` of a note segment.
pub const PT_NOTE: u32 = 4;
/// `p_flags`: executable.
pub const PF_X: u32 = 1;
/// `p_flags`: writable.
pub const PF_W: u32 = 2;
/// `p_flags`: readable.
pub const PF_R: u32 = 4;
/// Size of an `Elf64_Ehdr`.
pub const ELF64_EHDR_SIZE: usize = 64;
/// Size of an `Elf64_Phdr`.
pub const ELF64_PHDR_SIZE: usize = 56;

/// The generic fields of an `Elf64_Ehdr`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ehdr {
    /// `e_ident[EI_OSABI]`.
    pub osabi: u8,
    /// `e_type`.
    pub kind: u16,
    /// `e_machine`.
    pub machine: u16,
    /// `e_entry`.
    pub entry: u64,
    /// `e_phoff`.
    pub phoff: u64,
    /// `e_phentsize`.
    pub phentsize: u16,
    /// `e_phnum`.
    pub phnum: u16,
}

impl Ehdr {
    /// Parses a 64-bit little-endian ELF header; `None` for anything else.
    pub fn parse(b: &[u8]) -> Option<Self> {
        if b.get(..4)? != ELFMAG || *b.get(4)? != ELFCLASS64 || *b.get(5)? != ELFDATA2LSB {
            return None;
        }
        let u16_at = |off: usize| Some(u16::from_le_bytes(b.get(off..off + 2)?.try_into().ok()?));
        Some(Self {
            osabi: *b.get(7)?,
            kind: u16_at(16)?,
            machine: u16_at(18)?,
            entry: read_u64(b, 24)?,
            phoff: read_u64(b, 32)?,
            phentsize: u16_at(54)?,
            phnum: u16_at(56)?,
        })
    }
}

/// The fields of an `Elf64_Phdr`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Phdr {
    /// `p_type`.
    pub kind: u32,
    /// `p_flags` (`PF_*`).
    pub flags: u32,
    /// `p_offset`: where the segment's bytes are in the file.
    pub offset: u64,
    /// `p_vaddr`.
    pub vaddr: u64,
    /// `p_filesz`.
    pub filesz: u64,
    /// `p_memsz` (the rest, up to it, is zero-filled).
    pub memsz: u64,
}

/// The program headers of `image`, as far as they lie inside it.
pub fn phdrs<'a>(image: &'a [u8], ehdr: &Ehdr) -> impl Iterator<Item = Phdr> + 'a {
    let (phoff, phnum, phentsize) = (ehdr.phoff as usize, ehdr.phnum as usize, ehdr.phentsize as usize);
    (0..phnum).filter_map(move |i| {
        let ph = image.get(phoff.checked_add(i.checked_mul(phentsize)?)?..)?.get(..ELF64_PHDR_SIZE)?;
        Some(Phdr {
            kind: read_u32(ph, 0)?,
            flags: read_u32(ph, 4)?,
            offset: read_u64(ph, 8)?,
            vaddr: read_u64(ph, 16)?,
            filesz: read_u64(ph, 32)?,
            memsz: read_u64(ph, 40)?,
        })
    })
}

/// One ELF note (`Elf_Note` with its name and descriptor).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Note<'a> {
    /// The note's owner, without the NUL (e.g. `FreeBSD`).
    pub name: &'a [u8],
    /// `n_type`.
    pub kind: u32,
    /// The descriptor.
    pub desc: &'a [u8],
}

/// The notes of every `PT_NOTE` segment that lies inside `image` (the
/// first bytes of the file; notes beyond them are not seen).
pub fn notes<'a>(image: &'a [u8], ehdr: &Ehdr) -> impl Iterator<Item = Note<'a>> + 'a {
    let (phoff, phnum, phentsize) = (
        ehdr.phoff as usize,
        ehdr.phnum as usize,
        ehdr.phentsize as usize,
    );
    (0..phnum)
        .filter_map(move |i| {
            let ph = image
                .get(phoff.checked_add(i.checked_mul(phentsize)?)?..)?
                .get(..ELF64_PHDR_SIZE)?;
            if read_u32(ph, 0)? != PT_NOTE {
                return None;
            }
            let (off, size) = (read_u64(ph, 8)? as usize, read_u64(ph, 32)? as usize);
            image.get(off..off.checked_add(size)?)
        })
        .flat_map(|seg| {
            let mut off = 0;
            core::iter::from_fn(move || {
                let align4 = |n: usize| n.checked_add(3).map(|n| n & !3);
                let (namesz, descsz, kind) = (
                    read_u32(seg, off)? as usize,
                    read_u32(seg, off + 4)? as usize,
                    read_u32(seg, off + 8)?,
                );
                let name_at = off + 12;
                let desc_at = name_at.checked_add(align4(namesz)?)?;
                let name = seg.get(name_at..name_at + namesz)?;
                let desc = seg.get(desc_at..desc_at.checked_add(descsz)?)?;
                off = desc_at + align4(descsz)?;
                let name = name.strip_suffix(&[0]).unwrap_or(name);
                Some(Note { name, kind, desc })
            })
        })
}

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

    /// A minimal ELF64 executable header with one PT_NOTE segment holding
    /// the notes in `notes` (name, type, desc).
    pub(crate) fn exe(osabi: u8, machine: u16, notes: &[(&[u8], u32, &[u8])]) -> Vec<u8> {
        let mut seg = Vec::new();
        for (name, kind, desc) in notes {
            let mut n = name.to_vec();
            n.push(0);
            seg.extend((n.len() as u32).to_le_bytes());
            seg.extend((desc.len() as u32).to_le_bytes());
            seg.extend(kind.to_le_bytes());
            seg.extend(&n);
            seg.resize(seg.len().next_multiple_of(4), 0);
            seg.extend(*desc);
            seg.resize(seg.len().next_multiple_of(4), 0);
        }
        let mut b = vec![0u8; ELF64_EHDR_SIZE + ELF64_PHDR_SIZE];
        b[..4].copy_from_slice(ELFMAG);
        b[4] = ELFCLASS64;
        b[5] = ELFDATA2LSB;
        b[7] = osabi;
        b[16..18].copy_from_slice(&ET_EXEC.to_le_bytes());
        b[18..20].copy_from_slice(&machine.to_le_bytes());
        b[32..40].copy_from_slice(&(ELF64_EHDR_SIZE as u64).to_le_bytes());
        b[54..56].copy_from_slice(&(ELF64_PHDR_SIZE as u16).to_le_bytes());
        b[56..58].copy_from_slice(&1u16.to_le_bytes());
        let ph = ELF64_EHDR_SIZE;
        b[ph..ph + 4].copy_from_slice(&PT_NOTE.to_le_bytes());
        let seg_off = b.len() as u64;
        b[ph + 8..ph + 16].copy_from_slice(&seg_off.to_le_bytes());
        b[ph + 32..ph + 40].copy_from_slice(&(seg.len() as u64).to_le_bytes());
        b.extend(seg);
        b
    }

    #[test]
    fn parses_header_and_notes() {
        let b = exe(
            9,
            62,
            &[
                (b"FreeBSD", 1, &1501000u32.to_le_bytes()),
                (b"GNU", 3, &[1, 2, 3, 4, 5]),
            ],
        );
        let e = Ehdr::parse(&b).unwrap();
        assert_eq!((e.osabi, e.kind, e.machine, e.phnum), (9, ET_EXEC, 62, 1));
        let n: Vec<_> = notes(&b, &e).collect();
        assert_eq!(n.len(), 2);
        assert_eq!(
            (n[0].name, n[0].kind, n[0].desc),
            (&b"FreeBSD"[..], 1, &1501000u32.to_le_bytes()[..])
        );
        assert_eq!((n[1].name, n[1].desc.len()), (&b"GNU"[..], 5));
    }

    #[test]
    fn lists_program_headers() {
        let b = exe(9, 62, &[(b"FreeBSD", 1, &[0; 4])]);
        let e = Ehdr::parse(&b).unwrap();
        let p: Vec<Phdr> = phdrs(&b, &e).collect();
        assert_eq!(p.len(), 1);
        assert_eq!((p[0].kind, p[0].offset), (PT_NOTE, b.len() as u64 - p[0].filesz));
        assert_eq!(phdrs(&b[..70], &e).count(), 0);
    }

    #[test]
    fn rejects_non_elf64_and_survives_truncation() {
        let b = exe(0, 62, &[(b"Nova", 1, &[0; 4])]);
        assert!(Ehdr::parse(&b[..10]).is_none());
        let mut bad = b.clone();
        bad[4] = 1; // ELFCLASS32
        assert!(Ehdr::parse(&bad).is_none());
        let e = Ehdr::parse(&b).unwrap();
        for cut in 0..b.len() {
            let _ = notes(&b[..cut], &e).count();
        }
    }

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
