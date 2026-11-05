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


//! `compat_freebsd_ufs`: FreeBSD's file system, UFS2 (`sys/ufs/ffs` and
//! `sys/ufs/ufs`), read-only, on an image in memory (the memory disk the
//! loader preloads). It implements the kernel's [`VfsOps`]; `sys/conf`
//! registers it as `ufs` ([`UFS_VFSCONF`]), so `vfs.root.mountfrom` may
//! name it.
//!
//! The layout, from FreeBSD's `ufs/ffs/fs.h` and `ufs/ufs/dinode.h`
//! (offsets checked against them with the host's compiler):
//!
//! * the superblock at 64 KiB (`SBLOCK_UFS2`), magic `0x19540119`;
//! * cylinder groups of `fs_fpg` fragments, each with `fs_ipg` inodes at
//!   fragment `fs_iblkno` of the group;
//! * 256-byte inodes (`ufs2_dinode`): 12 direct block addresses, then
//!   single, double and triple indirect blocks of 64-bit addresses, in
//!   fragments; a short symbolic link is kept in the addresses' place;
//! * directories of `struct direct` records, in 512-byte chunks.
//!
//! 100% safe Rust: the image is a byte slice, and every read is checked.
//!
//! TODO(ufs): writing (soft updates, journal), several mounts, extended
//! attributes, UFS1.

#![no_std]
#![forbid(unsafe_code)]

use framework::sync::OnceLock;
use kern::vfs_mount::VfsConf;
use kern::vnode::{DirEnt, PathConf, StatFs, VAttr, VTime, VType, VfsOps};
use libkern::conv::{u64_from_usize, usize_from_u64};
use sys::errno::{Errno, KResult};

/// Where the superblock is (`SBLOCK_UFS2`).
const SBLOCK_UFS2: u64 = 65536;
/// `FS_UFS2_MAGIC`.
const FS_UFS2_MAGIC: u32 = 0x1954_0119;
/// Size of an inode (`sizeof(struct ufs2_dinode)`).
const DINODE_SIZE: u64 = 256;
/// Direct block addresses (`UFS_NDADDR`).
const NDADDR: usize = 12;
/// The root directory's inode (`UFS_ROOTINO`).
const ROOTINO: u64 = 2;
/// Longest name (`UFS_MAXNAMLEN`).
const MAXNAMLEN: usize = 255;

/// A mounted UFS2 file system (`struct ufsmount` with its `struct fs`).
pub struct Ufs {
    img: &'static [u8],
    bsize: u64,
    fsize: u64,
    frag: u64,
    iblkno: u64,
    ipg: u64,
    fpg: u64,
    ncg: u64,
    inopb: u64,
    nindir: u64,
    maxsymlinklen: u64,
    dsize: u64,
    nbfree: u64,
    nffree: u64,
    nifree: u64,
    id: u64,
}

fn le<const N: usize>(b: &[u8], off: usize) -> KResult<[u8; N]> {
    b.get(off..off + N).and_then(|s| s.try_into().ok()).ok_or(Errno::EIO)
}

fn u16_at(b: &[u8], off: usize) -> KResult<u16> {
    le(b, off).map(u16::from_le_bytes)
}

fn u32_at(b: &[u8], off: usize) -> KResult<u32> {
    le(b, off).map(u32::from_le_bytes)
}

fn u64_at(b: &[u8], off: usize) -> KResult<u64> {
    le(b, off).map(u64::from_le_bytes)
}

fn i64_at(b: &[u8], off: usize) -> KResult<i64> {
    le(b, off).map(i64::from_le_bytes)
}

/// An inode (`struct ufs2_dinode`), the fields read.
struct Dinode {
    mode: u16,
    nlink: u16,
    uid: u32,
    gid: u32,
    size: u64,
    blocks: u64,
    atime: VTime,
    mtime: VTime,
    ctime: VTime,
    birthtime: VTime,
    generation: u32,
    flags: u32,
    db: [u64; NDADDR],
    ib: [u64; 3],
    /// The 120 bytes of `di_db` and `di_ib`: a short link's target.
    addrs: &'static [u8],
}

impl Dinode {
    fn kind(&self) -> KResult<VType> {
        Ok(match self.mode & 0o170_000 {
            0o010_000 => VType::Fifo,
            0o020_000 => VType::Chr,
            0o040_000 => VType::Dir,
            0o060_000 => VType::Blk,
            0o100_000 => VType::Reg,
            0o120_000 => VType::Lnk,
            0o140_000 => VType::Sock,
            _ => return Err(Errno::EIO),
        })
    }
}

impl Ufs {
    /// Reads the superblock of `img` (`ffs_mountfs`, `ffs_sbget`).
    fn new(img: &'static [u8]) -> KResult<Self> {
        let sb = img.get(usize_from_u64(SBLOCK_UFS2)..).ok_or(Errno::EINVAL)?;
        if u32_at(sb, 1372)? != FS_UFS2_MAGIC {
            return Err(Errno::EINVAL);
        }
        let get = |off| u32_at(sb, off).map(u64::from);
        let fs = Ufs {
            img,
            bsize: get(48)?,
            fsize: get(52)?,
            frag: get(56)?,
            iblkno: get(16)?,
            ncg: get(44)?,
            inopb: get(120)?,
            nindir: get(116)?,
            ipg: get(184)?,
            fpg: get(188)?,
            maxsymlinklen: get(1320)?,
            dsize: u64_at(sb, 1088)?,
            nbfree: u64_at(sb, 1008 + 8)?,
            nifree: u64_at(sb, 1008 + 16)?,
            nffree: u64_at(sb, 1008 + 24)?,
            id: u64_at(sb, 144)?,
        };
        // What the rest relies on (`ffs_sbget`'s checks, in short).
        let sane = fs.fsize.is_power_of_two()
            && fs.bsize.is_power_of_two()
            && fs.fsize >= 512
            && fs.bsize >= fs.fsize
            && fs.bsize <= 65536
            && fs.frag == fs.bsize / fs.fsize
            && fs.inopb == fs.bsize / DINODE_SIZE
            && fs.nindir == fs.bsize / 8
            && fs.ipg > 0
            && fs.fpg > 0
            && fs.ncg > 0;
        if !sane {
            return Err(Errno::EINVAL);
        }
        Ok(fs)
    }

    /// `len` bytes of the image at byte `off`.
    fn bytes(&self, off: u64, len: usize) -> KResult<&'static [u8]> {
        let off = usize::try_from(off).map_err(|_| Errno::EIO)?;
        self.img.get(off..off.checked_add(len).ok_or(Errno::EIO)?).ok_or(Errno::EIO)
    }

    /// Inode `ino` (`ffs_vgetf`): in group `ino / ipg`, block
    /// `ino_to_fsba`, slot `ino_to_fsbo`.
    fn inode(&self, ino: u64) -> KResult<Dinode> {
        if ino < ROOTINO || ino >= self.ncg * self.ipg {
            return Err(Errno::ENOENT);
        }
        let cg = ino / self.ipg;
        let fsba = cg * self.fpg + self.iblkno + ((ino % self.ipg) / self.inopb) * self.frag;
        let d = self.bytes(fsba * self.fsize + (ino % self.inopb) * DINODE_SIZE, 256)?;
        let time = |s, ns| -> KResult<VTime> { Ok(VTime { sec: i64_at(d, s)?, nsec: i64::from(u32_at(d, ns)?) }) };
        let mut db = [0u64; NDADDR];
        for (i, a) in db.iter_mut().enumerate() {
            *a = u64_at(d, 112 + 8 * i)?;
        }
        let mut ib = [0u64; 3];
        for (i, a) in ib.iter_mut().enumerate() {
            *a = u64_at(d, 208 + 8 * i)?;
        }
        Ok(Dinode {
            mode: u16_at(d, 0)?,
            nlink: u16_at(d, 2)?,
            uid: u32_at(d, 4)?,
            gid: u32_at(d, 8)?,
            size: u64_at(d, 16)?,
            blocks: u64_at(d, 24)?,
            atime: time(32, 68)?,
            mtime: time(40, 64)?,
            ctime: time(48, 72)?,
            birthtime: time(56, 76)?,
            generation: u32_at(d, 80)?,
            flags: u32_at(d, 88)?,
            db,
            ib,
            addrs: d.get(112..232).ok_or(Errno::EIO)?,
        })
    }

    /// Entry `i` of indirect block `blk` (0: a hole).
    fn indir(&self, blk: u64, i: u64) -> KResult<u64> {
        if blk == 0 {
            return Ok(0);
        }
        u64_at(self.bytes(blk * self.fsize + i * 8, 8)?, 0)
    }

    /// The fragment holding logical block `lbn` of `d` (`ufs_bmaparray`);
    /// 0 for a hole.
    fn bmap(&self, d: &Dinode, lbn: u64) -> KResult<u64> {
        let n = self.nindir;
        if let Some(&a) = usize::try_from(lbn).ok().and_then(|i| d.db.get(i)) {
            return Ok(a);
        }
        let mut l = lbn - u64_from_usize(NDADDR);
        if l < n {
            return self.indir(d.ib[0], l);
        }
        l -= n;
        if l < n * n {
            return self.indir(self.indir(d.ib[1], l / n)?, l % n);
        }
        l -= n * n;
        if l < n * n * n {
            let mid = self.indir(d.ib[2], l / (n * n))?;
            return self.indir(self.indir(mid, (l / n) % n)?, l % n);
        }
        Err(Errno::EIO)
    }

    /// Reads `buf.len()` bytes of `d` at `off` (`ffs_read`), up to its end.
    fn read_inode(&self, d: &Dinode, off: u64, buf: &mut [u8]) -> KResult<usize> {
        if off >= d.size {
            return Ok(0);
        }
        let total = usize_from_u64((d.size - off).min(u64_from_usize(buf.len())));
        let mut done = 0;
        while done < total {
            let pos = off + u64_from_usize(done);
            let (lbn, boff) = (pos / self.bsize, pos % self.bsize);
            let n = usize_from_u64((self.bsize - boff).min(u64_from_usize(total - done)));
            let dst = buf.get_mut(done..done + n).ok_or(Errno::EIO)?;
            match self.bmap(d, lbn)? {
                0 => dst.fill(0),
                frag => dst.copy_from_slice(self.bytes(frag * self.fsize + boff, n)?),
            }
            done += n;
        }
        Ok(total)
    }
}

/// `DT_*` to a type.
fn dtype(t: u8) -> Option<VType> {
    Some(match t {
        1 => VType::Fifo,
        2 => VType::Chr,
        4 => VType::Dir,
        6 => VType::Blk,
        8 => VType::Reg,
        10 => VType::Lnk,
        12 => VType::Sock,
        _ => return None,
    })
}

impl VfsOps for Ufs {
    fn root(&self) -> u64 {
        ROOTINO
    }

    fn getattr(&self, ino: u64) -> KResult<VAttr> {
        let d = self.inode(ino)?;
        let kind = d.kind()?;
        Ok(VAttr {
            kind,
            perm: d.mode & 0o7777,
            nlink: u64::from(d.nlink),
            uid: d.uid,
            gid: d.gid,
            ino,
            size: d.size,
            blocks: d.blocks,
            blksize: u32::try_from(self.bsize).unwrap_or(u32::MAX),
            // `di_rdev` shares `di_db[0]`.
            rdev: if matches!(kind, VType::Chr | VType::Blk) { d.db[0] & 0xffff_ffff } else { 0 },
            atime: d.atime,
            mtime: d.mtime,
            ctime: d.ctime,
            birthtime: d.birthtime,
            flags: d.flags,
            generation: d.generation,
        })
    }

    fn lookup(&self, dir: u64, name: &[u8]) -> KResult<u64> {
        let mut found = None;
        self.readdir(dir, 0, &mut |e: &DirEnt<'_>| {
            if e.name == name {
                found = Some(e.ino);
                return false;
            }
            true
        })?;
        found.ok_or(Errno::ENOENT)
    }

    fn read(&self, ino: u64, off: u64, buf: &mut [u8]) -> KResult<usize> {
        let d = self.inode(ino)?;
        if d.kind()? == VType::Dir {
            return Err(Errno::EISDIR);
        }
        self.read_inode(&d, off, buf)
    }

    fn readlink(&self, ino: u64, buf: &mut [u8]) -> KResult<usize> {
        let d = self.inode(ino)?;
        if d.kind()? != VType::Lnk {
            return Err(Errno::EINVAL);
        }
        // A short link is kept in the inode (`ufs_readlink`).
        if d.size < self.maxsymlinklen && d.blocks == 0 {
            let n = usize_from_u64(d.size).min(buf.len());
            buf.get_mut(..n).ok_or(Errno::EIO)?.copy_from_slice(d.addrs.get(..n).ok_or(Errno::EIO)?);
            return Ok(n);
        }
        self.read_inode(&d, 0, buf)
    }

    fn readdir(&self, dir: u64, off: u64, f: &mut dyn FnMut(&DirEnt<'_>) -> bool) -> KResult<u64> {
        let d = self.inode(dir)?;
        if d.kind()? != VType::Dir {
            return Err(Errno::ENOTDIR);
        }
        let mut pos = off;
        let mut name = [0u8; MAXNAMLEN];
        while pos < d.size {
            // `struct direct`: d_ino, d_reclen, d_type, d_namlen, d_name.
            let mut head = [0u8; 8];
            if self.read_inode(&d, pos, &mut head)? < head.len() {
                break;
            }
            let ino = u64::from(u32_at(&head, 0)?);
            let reclen = u16_at(&head, 4)?;
            let namlen = usize::from(head[7]);
            if reclen < 8 || namlen > MAXNAMLEN {
                return Err(Errno::EIO);
            }
            let next = pos + u64::from(reclen);
            if ino != 0 {
                let n = name.get_mut(..namlen).ok_or(Errno::EIO)?;
                self.read_inode(&d, pos + 8, n)?;
                if !f(&DirEnt { ino, kind: dtype(head[6]), name: n, next }) {
                    return Ok(pos);
                }
            }
            pos = next;
        }
        Ok(pos)
    }

    fn statfs(&self) -> StatFs {
        let bfree = self.nbfree * self.frag + self.nffree;
        StatFs {
            bsize: self.fsize,
            iosize: self.bsize,
            blocks: self.dsize,
            bfree,
            // TODO(ufs): the reserve (`fs_minfree`).
            bavail: bfree,
            files: (self.ncg * self.ipg).saturating_sub(ROOTINO),
            ffree: self.nifree,
            namemax: 255,
            fsid: self.id,
        }
    }

    fn pathconf(&self, name: PathConf) -> KResult<i64> {
        match name {
            // `UFS_LINK_MAX`.
            PathConf::LinkMax => Ok(65530),
            PathConf::NameMax => Ok(255),
            PathConf::FileSizeBits => Ok(64),
            // Not enabled on this mount (`MNT_ACLS`, `MNT_NFS4ACLS`).
            PathConf::AclExtended | PathConf::AclNfs4 => Ok(0),
        }
    }
}

/// The mounted instance (one mount for now).
static MOUNT: OnceLock<Ufs> = OnceLock::new();

/// Mounts the UFS2 image `img` (`ffs_mount`), read-only.
fn ufs_mount(img: &'static [u8]) -> KResult<&'static dyn VfsOps> {
    MOUNT.set(Ufs::new(img)?).map_err(|_| Errno::EBUSY)?;
    let ufs: &'static Ufs = MOUNT.get().ok_or(Errno::EIO)?;
    Ok(ufs)
}

/// The file system type, for `sys/conf` to register (`VFS_SET(ufs_vfsops,
/// ufs, 0)`).
pub static UFS_VFSCONF: VfsConf = VfsConf { name: "ufs", mount: ufs_mount };

