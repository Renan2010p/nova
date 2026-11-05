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


//! Loading a kernel and entering it (`bi_load`, `elf64_exec`): the
//! segments, the environment, the preload metadata, the memory map, then
//! out of the firmware and into the kernel through the trampoline.

use core::fmt::Write as _;

use common::config::{Entry, boot_path};
use efi::{Console, Error, Pages};
use freebsd::env::Env;
use libkern::conv::{u64_from_usize, usize_from_u64};
use sys::linker::{
    MODINFO_ADDR, MODINFO_NAME, MODINFO_SIZE, MODINFO_TYPE, MODINFOMD_ELFHDR, MODINFOMD_ENVP, MODINFOMD_FW_HANDLE,
    MODINFOMD_HOWTO, MODINFOMD_KERNEND, PreloadWriter,
};

use crate::{MACHINE_ARCH, MODINFOMD_EFI_ARCH, MODINFOMD_EFI_FB, MODINFOMD_EFI_MAP, MODINFOMD_MODULEP, Trampoline, virt_to_phys};

macro_rules! printf {
    ($($arg:tt)*) => {{
        let _ = write!(Console, $($arg)*);
    }};
}

const PAGE: u64 = 4096;
/// Room after the kernel for the environment and the metadata.
const EXTRA_PAGES: usize = 64;

fn round_page(x: u64) -> u64 {
    x.div_ceil(PAGE) * PAGE
}

/// Why booting stopped (printed with `{:?}`).
#[derive(Debug)]
pub enum Fail {
    /// The entry has no `path` on the boot volume.
    NoPath,
    /// The firmware failed.
    Efi(Error),
    /// The kernel is not a loadable ELF file.
    Kernel(common::load_elf::LoadError),
    /// The environment is full.
    Env,
    /// The metadata does not fit.
    Metadata,
    /// The kernel's place overlaps the loader's memory.
    Overlap,
}

impl From<Error> for Fail {
    fn from(e: Error) -> Self {
        Fail::Efi(e)
    }
}

impl From<freebsd::env::Full> for Fail {
    fn from(_: freebsd::env::Full) -> Self {
        Fail::Env
    }
}

/// Adds a file of `name=value` lines to the environment, if it exists.
fn apply_file(path: &str, env: &mut Env<'_>) -> Result<(), Fail> {
    if let Ok((pages, len)) = efi::read_file(path) {
        let text = core::str::from_utf8(pages.bytes().get(..len).unwrap_or(&[])).unwrap_or("");
        freebsd::conf::apply(text, env)?;
        printf!("Loading {}\n", path);
    }
    Ok(())
}

/// FreeBSD's `acpi_detect`: where the kernel finds ACPI.
fn acpi_env(env: &mut Env<'_>) -> Result<(), Fail> {
    let Some(rsdp) = efi::config_table(&efi::ACPI_20_TABLE_GUID).or_else(|| efi::config_table(&efi::ACPI_TABLE_GUID)) else {
        return Ok(());
    };
    let mut b = [0u8; 64];
    let mut s = Buf::new(&mut b);
    let _ = write!(s, "0x{rsdp:016x}");
    env.set("acpi.rsdp", s.as_str())?;
    Ok(())
}

/// A `fmt::Write` into a byte buffer.
struct Buf<'a> {
    b: &'a mut [u8],
    n: usize,
}

impl<'a> Buf<'a> {
    fn new(b: &'a mut [u8]) -> Self {
        Self { b, n: 0 }
    }
    fn as_str(&self) -> &str {
        core::str::from_utf8(self.b.get(..self.n).unwrap_or(&[])).unwrap_or("")
    }
}

impl core::fmt::Write for Buf<'_> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let end = self.n + s.len();
        self.b.get_mut(self.n..end).ok_or(core::fmt::Error)?.copy_from_slice(s.as_bytes());
        self.n = end;
        Ok(())
    }
}

/// Boots the kernel of a `protocol: freebsd` entry: `path` (or
/// `kernel_path`), with `boot`'s arguments in `cmdline`. Returns only if
/// it could not.
pub fn boot(entry: &Entry<'_>) -> Result<core::convert::Infallible, Fail> {
    let kernel_path = entry.get("path").or_else(|| entry.get("kernel_path")).and_then(boot_path).ok_or(Fail::NoPath)?;
    // The environment, as the loader's scripts would build it.
    let mut env_pages = Pages::alloc(16, None)?;
    let mut env = Env::new(env_pages.bytes_mut());
    env.set("kernelname", kernel_path)?;
    apply_file("/boot/device.hints", &mut env)?;
    apply_file("/boot/loader.conf", &mut env)?;
    freebsd::howto::console_to_env(&mut env)?;
    acpi_env(&mut env)?;
    // `bi_getboothowto`: the arguments' flags and the environment's.
    let args = freebsd::howto::boot_parse_cmdline(entry.get("cmdline").unwrap_or(""), &mut env)?;
    freebsd::howto::boot_howto_to_env(args, &mut env)?;
    let howto = args | freebsd::howto::boot_env_to_howto(&env);

    printf!("Loading {}\n", kernel_path);
    let (file_pages, file_len) = efi::read_file(kernel_path)?;
    let file = file_pages.bytes().get(..file_len).ok_or(Fail::Kernel(common::load_elf::LoadError::NotElf))?;
    let kernel = common::load_elf::layout(file, virt_to_phys(0)).map_err(Fail::Kernel)?;
    let v2p = virt_to_phys(kernel.entry);
    let kernel = common::load_elf::layout(file, &v2p).map_err(Fail::Kernel)?;
    printf!(
        "{} text at 0x{:x}-0x{:x}, entry 0x{:x}\n",
        kernel_path, kernel.phys_start, kernel.phys_end, kernel.entry
    );

    // The staging area: the kernel, then the environment, then the metadata
    // and the memory map, all copied to `phys_start` at the end.
    let span = round_page(kernel.phys_end - kernel.phys_start);
    let mut staging = Pages::alloc(usize_from_u64(span / PAGE) + EXTRA_PAGES, None)?;
    let tramp = Trampoline::prepare()?;
    let dst = (kernel.phys_start, kernel.phys_start + u64_from_usize(staging.len()));
    let overlaps = |(a, b): (u64, u64)| a < dst.1 && dst.0 < b;
    if overlaps((staging.addr(), staging.addr() + u64_from_usize(staging.len()))) || tramp.ranges().into_iter().any(overlaps) {
        return Err(Fail::Overlap);
    }
    let staging_addr = staging.addr();
    let area = staging.bytes_mut();
    common::load_elf::load(file, &kernel, &v2p, area).map_err(Fail::Kernel)?;

    let env_off = usize_from_u64(span);
    let env_bytes = env.as_kernel_bytes()?;
    area.get_mut(env_off..env_off + env_bytes.len()).ok_or(Fail::Env)?.copy_from_slice(env_bytes);
    let modulep_off = usize_from_u64(round_page(u64_from_usize(env_off + env_bytes.len())));
    let phys = |off: usize| kernel.phys_start + u64_from_usize(off);

    let fb = efi::framebuffer().ok();
    if let Some(f) = fb {
        printf!("EFI framebuffer information:\naddr, size     0x{:x}, 0x{:x}\ndimensions     {} x {}\nstride         {}\n", f.addr, f.size, f.width, f.height, f.stride);
    }
    printf!("Start @ 0x{:x} ...\n", kernel.entry);

    // The memory map's size now, with room for it to grow until the end.
    let (map_room, desc_size) = {
        let probe = area.get_mut(modulep_off..).ok_or(Fail::Metadata)?;
        let half = probe.len() / 2;
        let m = efi::memory_map(probe.get_mut(half..).ok_or(Fail::Metadata)?)?;
        (m.descriptors.len() + 16 * m.descriptor_size, m.descriptor_size)
    };
    let _ = desc_size;

    let (md_len, kernend_at, map_at) = {
        let md = area.get_mut(modulep_off..).ok_or(Fail::Metadata)?;
        let mut w = PreloadWriter::new(md);
        let ok = || Fail::Metadata;
        let mut name = [0u8; 256];
        let name = name.get_mut(..=kernel_path.len()).ok_or_else(ok)?;
        name.get_mut(..kernel_path.len()).ok_or_else(ok)?.copy_from_slice(kernel_path.as_bytes());
        w.record(MODINFO_NAME, name).ok_or_else(ok)?;
        w.record(MODINFO_TYPE, b"elf kernel\0").ok_or_else(ok)?;
        w.record(MODINFO_ADDR, &kernel.phys_start.to_ne_bytes()).ok_or_else(ok)?;
        w.record(MODINFO_SIZE, &(kernel.phys_end - kernel.phys_start).to_ne_bytes()).ok_or_else(ok)?;
        w.metadata(MODINFOMD_ELFHDR, file.get(..64).ok_or_else(ok)?).ok_or_else(ok)?;
        w.metadata(MODINFOMD_HOWTO, &i32::try_from(howto).unwrap_or(0).to_ne_bytes()).ok_or_else(ok)?;
        w.metadata(MODINFOMD_ENVP, &phys(env_off).to_ne_bytes()).ok_or_else(ok)?;
        let kernend_at = w.metadata(MODINFOMD_KERNEND, &0u64.to_ne_bytes()).ok_or_else(ok)?;
        w.metadata(MODINFOMD_MODULEP, &phys(modulep_off).to_ne_bytes()).ok_or_else(ok)?;
        w.metadata(MODINFOMD_FW_HANDLE, &efi::system_table_addr().to_ne_bytes()).ok_or_else(ok)?;
        let mut arch = [0u8; 8];
        arch[..MACHINE_ARCH.len()].copy_from_slice(MACHINE_ARCH.as_bytes());
        w.metadata(MODINFOMD_EFI_ARCH, &arch[..=MACHINE_ARCH.len()]).ok_or_else(ok)?;
        if let Some(f) = fb {
            let mut b = [0u8; 44];
            b[0..8].copy_from_slice(&f.addr.to_ne_bytes());
            b[8..16].copy_from_slice(&f.size.to_ne_bytes());
            b[16..20].copy_from_slice(&f.height.to_ne_bytes());
            b[20..24].copy_from_slice(&f.width.to_ne_bytes());
            b[24..28].copy_from_slice(&f.stride.to_ne_bytes());
            for (i, m) in f.masks.iter().enumerate() {
                b[28 + 4 * i..32 + 4 * i].copy_from_slice(&m.to_ne_bytes());
            }
            w.metadata(MODINFOMD_EFI_FB, &b).ok_or_else(ok)?;
        }
        // struct efi_map_header, rounded to 16 bytes, then the descriptors.
        let map_at = w.reserve(sys::linker::MODINFO_METADATA | MODINFOMD_EFI_MAP, 32 + map_room).ok_or_else(ok)?;
        let len = w.finish().ok_or_else(ok)?;
        (len, kernend_at, map_at)
    };
    let kernend = round_page(phys(modulep_off + md_len));
    let md = area.get_mut(modulep_off..).ok_or(Fail::Metadata)?;
    md.get_mut(kernend_at..kernend_at + 8).ok_or(Fail::Metadata)?.copy_from_slice(&kernend.to_ne_bytes());
    if kernend > dst.1 {
        return Err(Fail::Metadata);
    }

    // The final memory map, straight into its record, then no more
    // firmware: nothing may allocate in between.
    let map_buf = md.get_mut(map_at + 32..map_at + 32 + map_room).ok_or(Fail::Metadata)?;
    let map = efi::memory_map(map_buf)?;
    let (msize, dsize, dver) = (map.descriptors.len(), map.descriptor_size, map.descriptor_version);
    efi::exit_boot_services(&map)?;
    let hdr = md.get_mut(map_at..map_at + 32).ok_or(Fail::Metadata)?;
    hdr[0..8].copy_from_slice(&u64_from_usize(msize).to_ne_bytes());
    hdr[8..16].copy_from_slice(&u64_from_usize(dsize).to_ne_bytes());
    hdr[16..20].copy_from_slice(&dver.to_ne_bytes());

    tramp.enter(staging_addr, kernend - kernel.phys_start, kernel.phys_start, phys(modulep_off), kernend, kernel.entry)
}
