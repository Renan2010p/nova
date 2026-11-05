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

//! Preload metadata: the boot protocol between the loader and the kernel
//! (FreeBSD's `sys/linker.h` constants plus `kern/subr_module.c` lookups).
//!
//! The loader hands the kernel a list of records describing every file it
//! loaded (the kernel itself, modules, a `md_image`, ...). Each record is
//!
//! ```text
//! u32 type | u32 size | size bytes of data | padding to 8 bytes
//! ```
//!
//! A file's records start with `MODINFO_NAME` and run until the next
//! `MODINFO_NAME`; the list ends with a record whose type and size are 0.
//!
//! This is Nova's native boot protocol, not a FreeBSD compatibility shim:
//! Nova is booted by FreeBSD's loader(8) today, and the Nova stand will
//! speak the same protocol so that kernel and loader stay interchangeable.
//!
//! Machine-dependent record types (`MODINFOMD_SMAP`, `MODINFOMD_EFI_MAP`,
//! ...) are defined by each architecture, not here.

use libkern::conv::usize_from_u32;
use crate::bytes::{read_u32, read_u64, roundup2};

/// End of the list.
pub const MODINFO_END: u32 = 0x0000;
/// Name of the file (string).
pub const MODINFO_NAME: u32 = 0x0001;
/// Type of the file (string), e.g. [`KERNTYPE`].
pub const MODINFO_TYPE: u32 = 0x0002;
/// Address where the file was loaded.
pub const MODINFO_ADDR: u32 = 0x0003;
/// Size of the loaded file.
pub const MODINFO_SIZE: u32 = 0x0004;
/// The file was unloaded by the loader.
pub const MODINFO_EMPTY: u32 = 0x0005;
/// Arguments string.
pub const MODINFO_ARGS: u32 = 0x0006;
/// Flag: file-specific metadata; the low bits are a `MODINFOMD_*` value.
pub const MODINFO_METADATA: u32 = 0x8000;

/// ELF header of the file.
pub const MODINFOMD_ELFHDR: u32 = 0x0002;
/// Start of the loaded symbol table.
pub const MODINFOMD_SSYM: u32 = 0x0003;
/// End of the loaded symbol table.
pub const MODINFOMD_ESYM: u32 = 0x0004;
/// Address of `_DYNAMIC`.
pub const MODINFOMD_DYNAMIC: u32 = 0x0005;
/// Kernel environment (address of the `envp` strings).
pub const MODINFOMD_ENVP: u32 = 0x0006;
/// `boothowto` flags, see [`crate::reboot`].
pub const MODINFOMD_HOWTO: u32 = 0x0007;
/// End of everything the loader placed after the kernel.
pub const MODINFOMD_KERNEND: u32 = 0x0008;
/// Section header table.
pub const MODINFOMD_SHDR: u32 = 0x0009;
/// Address of `.ctors`.
pub const MODINFOMD_CTORS_ADDR: u32 = 0x000a;
/// Size of `.ctors`.
pub const MODINFOMD_CTORS_SIZE: u32 = 0x000b;
/// Firmware-dependent handle (e.g. the EFI system table).
pub const MODINFOMD_FW_HANDLE: u32 = 0x000c;
/// Buffer with keys handed over by the loader (GELI, ...).
pub const MODINFOMD_KEYBUF: u32 = 0x000d;
/// Console font.
pub const MODINFOMD_FONT: u32 = 0x000e;
/// Console splash screen.
pub const MODINFOMD_SPLASH: u32 = 0x000f;
/// Console shutdown splash screen.
pub const MODINFOMD_SHTDWNSPLASH: u32 = 0x0010;
/// Loader-internal flag: this record is not copied to the kernel.
pub const MODINFOMD_NOCOPY: u32 = 0x8000;

/// `MODINFO_TYPE` of the kernel image.
pub const KERNTYPE: &str = "elf kernel";
/// Older loaders tag a 64-bit kernel with this type instead.
pub const KERNTYPE64: &str = "elf64 kernel";

/// Records are padded to `sizeof(u_long)`.
const MD_ALIGN: usize = 8;
/// Size of the `type | size` header of a record.
const HDR_SIZE: usize = 8;

/// One preload record.
#[derive(Clone, Copy, Debug)]
pub struct Record<'a> {
    /// `MODINFO_*`, or `MODINFO_METADATA | MODINFOMD_*`.
    pub kind: u32,
    /// The record payload, without padding.
    pub data: &'a [u8],
}

impl<'a> Record<'a> {
    /// The `MODINFOMD_*` type, if this is a metadata record.
    pub fn md_type(&self) -> Option<u32> {
        (self.kind & MODINFO_METADATA != 0).then_some(self.kind & !MODINFO_METADATA)
    }

    /// The payload as a NUL-terminated string.
    pub fn as_str(&self) -> Option<&'a str> {
        let end = self
            .data
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(self.data.len());
        core::str::from_utf8(&self.data[..end]).ok()
    }

    /// The payload as a `u32` (`MD_FETCH(..., int)`).
    pub fn as_u32(&self) -> Option<u32> {
        read_u32(self.data, 0)
    }

    /// The payload as a `u64` (`MD_FETCH(..., vm_offset_t)`).
    pub fn as_u64(&self) -> Option<u64> {
        read_u64(self.data, 0)
    }
}

/// Parses the record at `off`; returns it and the offset of the next one.
fn parse(buf: &[u8], off: usize) -> Option<(Record<'_>, usize)> {
    let kind = read_u32(buf, off)?;
    let size = usize_from_u32(read_u32(buf, off + 4)?);
    if kind == MODINFO_END && size == 0 {
        return None;
    }
    let start = off.checked_add(HDR_SIZE)?;
    let end = start.checked_add(size)?;
    let data = buf.get(start..end)?;
    Some((Record { kind, data }, roundup2(end, MD_ALIGN)?))
}

/// Writes preload metadata into a buffer (the loader's `md_copymodules`):
/// records of a 32-bit type, a 32-bit size and the data, each padded to 8
/// bytes, ended by `MODINFO_END`. The reader is [`Preload`]; one format,
/// both sides of the boot protocol.
pub struct PreloadWriter<'a> {
    buf: &'a mut [u8],
    len: usize,
}

impl<'a> PreloadWriter<'a> {
    /// Writes into `buf`, from its start.
    pub fn new(buf: &'a mut [u8]) -> Self {
        Self { buf, len: 0 }
    }

    /// Appends a record. Returns the offset of its data in the buffer (to
    /// patch it later, e.g. `MODINFOMD_KERNEND`), or `None` if it does not
    /// fit.
    pub fn record(&mut self, kind: u32, data: &[u8]) -> Option<usize> {
        let at = self.reserve(kind, data.len())?;
        self.buf.get_mut(at..at + data.len())?.copy_from_slice(data);
        Some(at)
    }

    /// Appends a record of `size` zero bytes, to be filled in later.
    /// Returns the offset of its data.
    pub fn reserve(&mut self, kind: u32, size: usize) -> Option<usize> {
        let start = self.len;
        let data = start.checked_add(HDR_SIZE)?;
        let end = roundup2(data.checked_add(size)?, MD_ALIGN)?;
        let rec = self.buf.get_mut(start..end)?;
        rec.fill(0);
        rec.get_mut(..4)?.copy_from_slice(&kind.to_ne_bytes());
        rec.get_mut(4..8)?.copy_from_slice(&u32::try_from(size).ok()?.to_ne_bytes());
        self.len = end;
        Some(data)
    }

    /// A metadata record of the current module (`MODINFO_METADATA | md`).
    pub fn metadata(&mut self, md: u32, data: &[u8]) -> Option<usize> {
        self.record(MODINFO_METADATA | md, data)
    }

    /// The buffer, to patch reserved records.
    pub fn buf_mut(&mut self) -> &mut [u8] {
        self.buf
    }

    /// Ends the list (`MODINFO_END`) and returns its total size.
    pub fn finish(mut self) -> Option<usize> {
        self.reserve(MODINFO_END, 0)?;
        Some(self.len)
    }
}

/// Iterator over raw records. Stops at `MODINFO_END` or at the first
/// malformed record.
#[derive(Clone, Debug)]
pub struct Records<'a> {
    buf: &'a [u8],
    off: usize,
}

impl<'a> Iterator for Records<'a> {
    type Item = Record<'a>;

    fn next(&mut self) -> Option<Record<'a>> {
        match parse(self.buf, self.off) {
            Some((r, next)) => {
                self.off = next;
                Some(r)
            }
            None => {
                self.off = self.buf.len();
                None
            }
        }
    }
}

/// The whole preload metadata area handed over by the loader.
#[derive(Clone, Copy, Debug)]
pub struct Preload<'a> {
    buf: &'a [u8],
}

impl<'a> Preload<'a> {
    /// Wraps the metadata area. `buf` may extend past the end of the list;
    /// parsing stops at `MODINFO_END`.
    pub const fn new(buf: &'a [u8]) -> Self {
        Self { buf }
    }

    /// No metadata (kernel not started by a loader).
    pub const fn empty() -> Self {
        Self { buf: &[] }
    }

    /// Every record of every file, in order.
    pub fn records(&self) -> Records<'a> {
        Records {
            buf: self.buf,
            off: 0,
        }
    }

    /// Every file described by the metadata.
    pub fn modules(&self) -> Modules<'a> {
        Modules {
            buf: self.buf,
            off: 0,
        }
    }

    /// First file with the given `MODINFO_TYPE` (`preload_search_by_type`).
    pub fn search_by_type(&self, ty: &str) -> Option<Module<'a>> {
        self.modules().find(|m| m.type_name() == Some(ty))
    }

    /// The kernel's own records (`preload_kmdp`).
    pub fn kernel(&self) -> Option<Module<'a>> {
        self.search_by_type(KERNTYPE)
            .or_else(|| self.search_by_type(KERNTYPE64))
    }
}

/// Iterator over the files in the metadata.
#[derive(Clone, Debug)]
pub struct Modules<'a> {
    buf: &'a [u8],
    off: usize,
}

impl<'a> Iterator for Modules<'a> {
    type Item = Module<'a>;

    fn next(&mut self) -> Option<Module<'a>> {
        while let Some((r, next)) = parse(self.buf, self.off) {
            let here = self.off;
            self.off = next;
            if r.kind == MODINFO_NAME {
                return Some(Module {
                    buf: &self.buf[here..],
                });
            }
        }
        self.off = self.buf.len();
        None
    }
}

/// The records of one loaded file.
#[derive(Clone, Copy, Debug)]
pub struct Module<'a> {
    /// Starts at this file's `MODINFO_NAME` record.
    buf: &'a [u8],
}

impl<'a> Module<'a> {
    /// This file's records, from its `MODINFO_NAME` up to the next file.
    pub fn records(&self) -> impl Iterator<Item = Record<'a>> + 'a {
        let mut it = Records {
            buf: self.buf,
            off: 0,
        };
        let first = it.next();
        first
            .into_iter()
            .chain(it.take_while(|r| r.kind != MODINFO_NAME))
    }

    /// First record of the given kind (`preload_search_info`).
    pub fn search_info(&self, kind: u32) -> Option<Record<'a>> {
        self.records().find(|r| r.kind == kind)
    }

    /// Metadata record `MODINFO_METADATA | md` (`MD_FETCH`).
    pub fn md(&self, md: u32) -> Option<Record<'a>> {
        self.search_info(MODINFO_METADATA | md)
    }

    /// `MODINFO_NAME`, e.g. `/boot/kernel/kernel`.
    pub fn name(&self) -> Option<&'a str> {
        self.search_info(MODINFO_NAME)?.as_str()
    }

    /// `MODINFO_TYPE`, e.g. [`KERNTYPE`].
    pub fn type_name(&self) -> Option<&'a str> {
        self.search_info(MODINFO_TYPE)?.as_str()
    }

    /// `MODINFO_ADDR`.
    pub fn addr(&self) -> Option<u64> {
        self.search_info(MODINFO_ADDR)?.as_u64()
    }

    /// `MODINFO_SIZE`.
    pub fn size(&self) -> Option<u64> {
        self.search_info(MODINFO_SIZE)?.as_u64()
    }
}

#[cfg(test)]
#[allow(clippy::as_conversions, reason = "host test fixtures, not kernel code")]
mod tests {
    use super::*;

    fn push(buf: &mut Vec<u8>, kind: u32, data: &[u8]) {
        buf.extend_from_slice(&kind.to_ne_bytes());
        buf.extend_from_slice(&(data.len() as u32).to_ne_bytes());
        buf.extend_from_slice(data);
        while !buf.len().is_multiple_of(MD_ALIGN) {
            buf.push(0);
        }
    }

    #[test]
    fn writer_round_trips() {
        let mut buf = [0u8; 256];
        let mut w = PreloadWriter::new(&mut buf);
        w.record(MODINFO_NAME, b"/boot/kernel/kernel\0").unwrap();
        w.record(MODINFO_TYPE, b"elf kernel\0").unwrap();
        let at = w.metadata(MODINFOMD_KERNEND, &0u64.to_ne_bytes()).unwrap();
        w.buf_mut()[at..at + 8].copy_from_slice(&0x1234u64.to_ne_bytes());
        let len = w.finish().unwrap();
        let p = Preload::new(&buf[..len]);
        let k = p.kernel().unwrap();
        assert_eq!(k.name(), Some("/boot/kernel/kernel"));
        assert_eq!(k.md(MODINFOMD_KERNEND).and_then(|r| r.as_u64()), Some(0x1234));
    }

    fn sample() -> Vec<u8> {
        let mut b = Vec::new();
        push(&mut b, MODINFO_NAME, b"/boot/nova/kernel\0");
        push(&mut b, MODINFO_TYPE, b"elf kernel\0");
        push(
            &mut b,
            MODINFO_ADDR,
            &0xffff_ffff_8020_0000u64.to_ne_bytes(),
        );
        push(
            &mut b,
            MODINFO_METADATA | MODINFOMD_HOWTO,
            &0x800u32.to_ne_bytes(),
        );
        push(&mut b, MODINFO_NAME, b"/boot/kernel/zfs.ko\0");
        push(&mut b, MODINFO_TYPE, b"elf obj module\0");
        push(
            &mut b,
            MODINFO_METADATA | MODINFOMD_HOWTO,
            &0x2u32.to_ne_bytes(),
        );
        push(&mut b, MODINFO_END, &[]);
        b.extend_from_slice(&[0xAA; 32]); // garbage after the list
        b
    }

    #[test]
    fn finds_kernel_and_its_metadata() {
        let buf = sample();
        let p = Preload::new(&buf);
        let k = p.kernel().expect("kernel");
        assert_eq!(k.name(), Some("/boot/nova/kernel"));
        assert_eq!(k.addr(), Some(0xffff_ffff_8020_0000));
        assert_eq!(k.md(MODINFOMD_HOWTO).and_then(|r| r.as_u32()), Some(0x800));
    }

    #[test]
    fn module_records_stop_at_next_module() {
        let buf = sample();
        let p = Preload::new(&buf);
        let names: Vec<_> = p.modules().filter_map(|m| m.name()).collect();
        assert_eq!(names, ["/boot/nova/kernel", "/boot/kernel/zfs.ko"]);
        let module = p.search_by_type("elf obj module").unwrap();
        assert_eq!(module.md(MODINFOMD_HOWTO).and_then(|r| r.as_u32()), Some(0x2));
        assert_eq!(module.records().count(), 3);
    }

    #[test]
    fn stops_at_end_and_survives_truncation() {
        let buf = sample();
        assert_eq!(Preload::new(&buf).records().count(), 7);
        for cut in 0..buf.len() {
            let p = Preload::new(&buf[..cut]);
            let _ = p.records().count();
            let _ = p.kernel();
        }
        assert!(Preload::empty().kernel().is_none());
    }
}
