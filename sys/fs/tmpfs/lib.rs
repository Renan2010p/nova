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


//! `tmpfs`: a file system in memory (FreeBSD's `fs/tmpfs`, by its
//! behaviour; the format is Nova's own, as it is never stored). Files
//! are bytes in kernel memory, directories lists of names; everything
//! goes when it is unmounted or the system stops. It implements the
//! kernel's [`VfsOps`], writes included; `sys/conf` registers it as
//! `tmpfs`, which `mount -t tmpfs tmpfs /tmp` names.
//!
//! 100% safe Rust. One lock per mount: simple, not concurrent.
//!
//! TODO(tmpfs): size limits (`size=`), unmounting, page-backed storage
//! shared with `mmap`.

#![no_std]
#![forbid(unsafe_code)]

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use framework::mm::{KVec, M_WAITOK, MallocType};
use framework::sync::{Mutex, OnceLock};
use kern::vfs_mount::{MountFrom, VfsConf};
use kern::vnode::{DirEnt, NewNode, Owner, SetAttr, StatFs, VAttr, VTime, VType, VfsOps};
use libkern::conv::{u64_from_usize, usize_from_u64};
use sys::errno::{Errno, KResult};

/// tmpfs nodes and data (`M_TMPFSMNT`, `M_TMPFSNAME`).
static M_TMPFS: MallocType = MallocType::new("tmpfs");

/// The root directory's number.
const ROOT: u64 = 2;
/// Longest name.
const NAME_MAX: usize = 255;

/// A file (`struct tmpfs_node`).
struct Node {
    kind: VType,
    perm: u16,
    uid: u32,
    gid: u32,
    nlink: u64,
    flags: u32,
    atime: VTime,
    mtime: VTime,
    ctime: VTime,
    birthtime: VTime,
    /// A file's contents, or a link's target.
    data: KVec<u8>,
    /// A directory's entries: names and numbers.
    entries: KVec<(KVec<u8>, u64)>,
    /// A directory's parent.
    parent: u64,
}

/// A mounted tmpfs (`struct tmpfs_mount`): nodes by number (index).
pub struct Tmpfs {
    nodes: Mutex<KVec<Option<Node>>>,
    id: u64,
}

fn now() -> VTime {
    kern::vfs_syscalls::vfs_timestamp()
}

fn bytes(src: &[u8]) -> KResult<KVec<u8>> {
    let mut v = KVec::with_capacity(src.len(), &M_TMPFS, M_WAITOK)?;
    v.extend_from_slice(src, M_WAITOK)?;
    Ok(v)
}

fn node(nodes: &KVec<Option<Node>>, ino: u64) -> KResult<&Node> {
    nodes.get(usize_from_u64(ino)).and_then(Option::as_ref).ok_or(Errno::ENOENT)
}

fn node_mut(nodes: &mut KVec<Option<Node>>, ino: u64) -> KResult<&mut Node> {
    nodes.get_mut(usize_from_u64(ino)).and_then(Option::as_mut).ok_or(Errno::ENOENT)
}

fn dir_mut(nodes: &mut KVec<Option<Node>>, ino: u64) -> KResult<&mut Node> {
    let n = node_mut(nodes, ino)?;
    if n.kind != VType::Dir {
        return Err(Errno::ENOTDIR);
    }
    Ok(n)
}

fn find(n: &Node, name: &[u8]) -> Option<usize> {
    n.entries.iter().position(|(e, _)| &**e == name)
}

impl Tmpfs {
    fn new(id: u64) -> KResult<Self> {
        let mut nodes = KVec::new(&M_TMPFS);
        nodes.push(None, M_WAITOK)?;
        nodes.push(None, M_WAITOK)?;
        let t = now();
        nodes.push(
            Some(Node {
                kind: VType::Dir,
                perm: 0o1777,
                uid: 0,
                gid: 0,
                nlink: 2,
                flags: 0,
                atime: t,
                mtime: t,
                ctime: t,
                birthtime: t,
                data: KVec::new(&M_TMPFS),
                entries: KVec::new(&M_TMPFS),
                parent: ROOT,
            }),
            M_WAITOK,
        )?;
        Ok(Self { nodes: Mutex::new(nodes), id })
    }

    /// A free number, reusing removed ones.
    fn alloc(nodes: &mut KVec<Option<Node>>, n: Node) -> KResult<u64> {
        if let Some(i) = nodes.iter().skip(3).position(Option::is_none) {
            let i = i + 3;
            if let Some(slot) = nodes.get_mut(i) {
                *slot = Some(n);
            }
            return Ok(u64_from_usize(i));
        }
        nodes.push(Some(n), M_WAITOK)?;
        Ok(u64_from_usize(nodes.len() - 1))
    }

    /// One name fewer for `ino`; freed when it has none.
    fn unref(nodes: &mut KVec<Option<Node>>, ino: u64) {
        let gone = match node_mut(nodes, ino) {
            Ok(n) => {
                n.nlink = n.nlink.saturating_sub(1);
                n.ctime = now();
                n.nlink == 0 || (n.kind == VType::Dir && n.nlink <= 1)
            }
            Err(_) => false,
        };
        if gone && let Some(slot) = nodes.get_mut(usize_from_u64(ino)) {
            *slot = None;
        }
    }
}

impl VfsOps for Tmpfs {
    fn root(&self) -> u64 {
        ROOT
    }

    fn getattr(&self, ino: u64) -> KResult<VAttr> {
        let nodes = self.nodes.lock();
        let n = node(&nodes, ino)?;
        let size = match n.kind {
            VType::Dir => u64_from_usize((n.entries.len() + 2) * 32),
            _ => u64_from_usize(n.data.len()),
        };
        Ok(VAttr {
            kind: n.kind,
            perm: n.perm,
            nlink: n.nlink,
            uid: n.uid,
            gid: n.gid,
            ino,
            size,
            blocks: size.div_ceil(512),
            blksize: 4096,
            rdev: 0,
            atime: n.atime,
            mtime: n.mtime,
            ctime: n.ctime,
            birthtime: n.birthtime,
            flags: n.flags,
            generation: 0,
        })
    }

    fn lookup(&self, dir: u64, name: &[u8]) -> KResult<u64> {
        let nodes = self.nodes.lock();
        let d = node(&nodes, dir)?;
        if d.kind != VType::Dir {
            return Err(Errno::ENOTDIR);
        }
        match name {
            b"." => Ok(dir),
            b".." => Ok(d.parent),
            _ => find(d, name).and_then(|i| d.entries.get(i)).map(|e| e.1).ok_or(Errno::ENOENT),
        }
    }

    fn read(&self, ino: u64, off: u64, buf: &mut [u8]) -> KResult<usize> {
        let nodes = self.nodes.lock();
        let n = node(&nodes, ino)?;
        if n.kind == VType::Dir {
            return Err(Errno::EISDIR);
        }
        let off = usize::try_from(off).map_err(|_| Errno::EINVAL)?;
        let src = n.data.get(off..).unwrap_or(&[]);
        let k = src.len().min(buf.len());
        buf.get_mut(..k).ok_or(Errno::EIO)?.copy_from_slice(src.get(..k).ok_or(Errno::EIO)?);
        Ok(k)
    }

    fn readlink(&self, ino: u64, buf: &mut [u8]) -> KResult<usize> {
        let nodes = self.nodes.lock();
        let n = node(&nodes, ino)?;
        if n.kind != VType::Lnk {
            return Err(Errno::EINVAL);
        }
        let k = n.data.len().min(buf.len());
        buf.get_mut(..k).ok_or(Errno::EIO)?.copy_from_slice(n.data.get(..k).ok_or(Errno::EIO)?);
        Ok(k)
    }

    fn readdir(&self, dir: u64, off: u64, f: &mut dyn FnMut(&DirEnt<'_>) -> bool) -> KResult<u64> {
        let nodes = self.nodes.lock();
        let d = node(&nodes, dir)?;
        if d.kind != VType::Dir {
            return Err(Errno::ENOTDIR);
        }
        // Offsets: 0 ".", 1 "..", then the entries in order.
        let mut i = off;
        loop {
            let ent = match i {
                0 => DirEnt { ino: dir, kind: Some(VType::Dir), name: b".", next: 1 },
                1 => DirEnt { ino: d.parent, kind: Some(VType::Dir), name: b"..", next: 2 },
                k => {
                    let Some((name, ino)) = usize::try_from(k - 2).ok().and_then(|j| d.entries.get(j)) else {
                        return Ok(i);
                    };
                    let kind = node(&nodes, *ino).ok().map(|n| n.kind);
                    DirEnt { ino: *ino, kind, name, next: k + 1 }
                }
            };
            if !f(&ent) {
                return Ok(i);
            }
            i += 1;
        }
    }

    fn statfs(&self) -> StatFs {
        let nodes = self.nodes.lock();
        let used: u64 = nodes.iter().flatten().map(|n| u64_from_usize(n.data.len()).div_ceil(4096)).sum();
        let free = framework::mm::vm_phys::free_count();
        StatFs {
            bsize: 4096,
            iosize: 4096,
            blocks: used + free,
            bfree: free,
            bavail: free,
            files: u64_from_usize(nodes.len()) + free,
            ffree: free,
            namemax: 255,
            fsid: self.id,
        }
    }

    fn create(&self, dir: u64, name: &[u8], new: NewNode<'_>, owner: Owner) -> KResult<u64> {
        if name.len() > NAME_MAX {
            return Err(Errno::ENAMETOOLONG);
        }
        let mut nodes = self.nodes.lock();
        let d = dir_mut(&mut nodes, dir)?;
        if find(d, name).is_some() {
            return Err(Errno::EEXIST);
        }
        let t = now();
        let (kind, data, nlink) = match new {
            NewNode::File => (VType::Reg, KVec::new(&M_TMPFS), 1),
            NewNode::Dir => (VType::Dir, KVec::new(&M_TMPFS), 2),
            NewNode::Symlink(target) => (VType::Lnk, bytes(target)?, 1),
            NewNode::Fifo => (VType::Fifo, KVec::new(&M_TMPFS), 1),
            NewNode::Socket => (VType::Sock, KVec::new(&M_TMPFS), 1),
        };
        let perm = if kind == VType::Lnk { 0o755 } else { owner.perm };
        let n = Node {
            kind,
            perm,
            uid: owner.uid,
            gid: owner.gid,
            nlink,
            flags: 0,
            atime: t,
            mtime: t,
            ctime: t,
            birthtime: t,
            data,
            entries: KVec::new(&M_TMPFS),
            parent: dir,
        };
        let name = bytes(name)?;
        let ino = Self::alloc(&mut nodes, n)?;
        let d = dir_mut(&mut nodes, dir)?;
        d.entries.push((name, ino), M_WAITOK)?;
        d.mtime = t;
        d.ctime = t;
        if kind == VType::Dir {
            d.nlink += 1;
        }
        Ok(ino)
    }

    fn write(&self, ino: u64, off: u64, data: &[u8]) -> KResult<usize> {
        let mut nodes = self.nodes.lock();
        let n = node_mut(&mut nodes, ino)?;
        if n.kind != VType::Reg {
            return Err(Errno::EINVAL);
        }
        let off = usize::try_from(off).map_err(|_| Errno::EINVAL)?;
        let end = off.checked_add(data.len()).ok_or(Errno::EINVAL)?;
        while n.data.len() < end {
            n.data.push(0, M_WAITOK)?;
        }
        n.data.get_mut(off..end).ok_or(Errno::EIO)?.copy_from_slice(data);
        let t = now();
        n.mtime = t;
        n.ctime = t;
        Ok(data.len())
    }

    fn setattr(&self, ino: u64, a: &SetAttr) -> KResult<()> {
        let mut nodes = self.nodes.lock();
        let n = node_mut(&mut nodes, ino)?;
        if let Some(perm) = a.perm {
            n.perm = perm & 0o7777;
        }
        if let Some(uid) = a.uid {
            n.uid = uid;
        }
        if let Some(gid) = a.gid {
            n.gid = gid;
        }
        if let Some(size) = a.size {
            let size = usize::try_from(size).map_err(|_| Errno::EINVAL)?;
            if size < n.data.len() {
                while n.data.len() > size {
                    n.data.pop();
                }
            } else {
                while n.data.len() < size {
                    n.data.push(0, M_WAITOK)?;
                }
            }
            n.mtime = now();
        }
        if let Some(t) = a.atime {
            n.atime = t;
        }
        if let Some(t) = a.mtime {
            n.mtime = t;
        }
        if let Some(f) = a.flags {
            n.flags = f;
        }
        n.ctime = now();
        Ok(())
    }

    fn remove(&self, dir: u64, name: &[u8], dir_only: bool) -> KResult<()> {
        let mut nodes = self.nodes.lock();
        let d = dir_mut(&mut nodes, dir)?;
        let i = find(d, name).ok_or(Errno::ENOENT)?;
        let ino = d.entries.get(i).map(|e| e.1).ok_or(Errno::ENOENT)?;
        let victim = node(&nodes, ino)?;
        match (dir_only, victim.kind == VType::Dir) {
            (true, false) => return Err(Errno::ENOTDIR),
            (false, true) => return Err(Errno::EPERM),
            (true, true) if !victim.entries.is_empty() => return Err(Errno::ENOTEMPTY),
            _ => {}
        }
        let d = dir_mut(&mut nodes, dir)?;
        drop(d.entries.remove(i));
        let t = now();
        d.mtime = t;
        d.ctime = t;
        if dir_only {
            d.nlink = d.nlink.saturating_sub(1);
            if let Some(slot) = nodes.get_mut(usize_from_u64(ino)) {
                *slot = None;
            }
        } else {
            Self::unref(&mut nodes, ino);
        }
        Ok(())
    }

    fn rename(&self, fdir: u64, from: &[u8], tdir: u64, to: &[u8]) -> KResult<()> {
        if to.len() > NAME_MAX {
            return Err(Errno::ENAMETOOLONG);
        }
        let mut nodes = self.nodes.lock();
        let fd = dir_mut(&mut nodes, fdir)?;
        let fi = find(fd, from).ok_or(Errno::ENOENT)?;
        let ino = fd.entries.get(fi).map(|e| e.1).ok_or(Errno::ENOENT)?;
        let is_dir = node(&nodes, ino)?.kind == VType::Dir;
        // A directory may not move under itself.
        if is_dir {
            let mut up = tdir;
            while up != ROOT {
                if up == ino {
                    return Err(Errno::EINVAL);
                }
                up = node(&nodes, up)?.parent;
            }
        }
        // What `to` names goes, if it may.
        let td = dir_mut(&mut nodes, tdir)?;
        if let Some(ti) = find(td, to) {
            let old = td.entries.get(ti).map(|e| e.1).ok_or(Errno::ENOENT)?;
            if old == ino {
                return Ok(());
            }
            let o = node(&nodes, old)?;
            match (is_dir, o.kind == VType::Dir) {
                (true, false) => return Err(Errno::ENOTDIR),
                (false, true) => return Err(Errno::EISDIR),
                (true, true) if !o.entries.is_empty() => return Err(Errno::ENOTEMPTY),
                _ => {}
            }
            self.remove_locked(&mut nodes, tdir, to, is_dir)?;
        }
        let name = bytes(to)?;
        let fd = dir_mut(&mut nodes, fdir)?;
        let fi = find(fd, from).ok_or(Errno::ENOENT)?;
        drop(fd.entries.remove(fi));
        if is_dir {
            fd.nlink = fd.nlink.saturating_sub(1);
        }
        let td = dir_mut(&mut nodes, tdir)?;
        td.entries.push((name, ino), M_WAITOK)?;
        if is_dir {
            td.nlink += 1;
            node_mut(&mut nodes, ino)?.parent = tdir;
        }
        Ok(())
    }

    fn link(&self, dir: u64, name: &[u8], ino: u64) -> KResult<()> {
        let mut nodes = self.nodes.lock();
        if find(dir_mut(&mut nodes, dir)?, name).is_some() {
            return Err(Errno::EEXIST);
        }
        let n = node_mut(&mut nodes, ino)?;
        if n.kind == VType::Dir {
            return Err(Errno::EPERM);
        }
        n.nlink += 1;
        n.ctime = now();
        let name = bytes(name)?;
        dir_mut(&mut nodes, dir)?.entries.push((name, ino), M_WAITOK)?;
        Ok(())
    }
}

impl Tmpfs {
    /// [`VfsOps::remove`] with the lock held (for `rename`).
    fn remove_locked(&self, nodes: &mut KVec<Option<Node>>, dir: u64, name: &[u8], is_dir: bool) -> KResult<()> {
        let d = dir_mut(nodes, dir)?;
        let i = find(d, name).ok_or(Errno::ENOENT)?;
        let ino = d.entries.get(i).map(|e| e.1).ok_or(Errno::ENOENT)?;
        drop(d.entries.remove(i));
        if is_dir {
            d.nlink = d.nlink.saturating_sub(1);
            if let Some(slot) = nodes.get_mut(usize_from_u64(ino)) {
                *slot = None;
            }
        } else {
            Self::unref(nodes, ino);
        }
        Ok(())
    }
}

/// Most tmpfs mounts at once.
const MAX: usize = 8;
static MOUNTS: [OnceLock<Tmpfs>; MAX] = [const { OnceLock::new() }; MAX];
static NEXT: AtomicUsize = AtomicUsize::new(0);
static NEXT_ID: AtomicU64 = AtomicU64::new(0x7470_0000);

/// Mounts a new, empty tmpfs (`tmpfs_mount`).
fn tmpfs_mount(from: MountFrom) -> KResult<&'static dyn VfsOps> {
    let MountFrom::Nothing = from else { return Err(Errno::EINVAL) };
    let slot = MOUNTS.get(NEXT.fetch_add(1, Ordering::AcqRel)).ok_or(Errno::ENOMEM)?;
    slot.set(Tmpfs::new(NEXT_ID.fetch_add(1, Ordering::Relaxed))?).map_err(|_| Errno::EBUSY)?;
    let fs: &'static Tmpfs = slot.get().ok_or(Errno::EIO)?;
    Ok(fs)
}

/// The file system type, for `sys/conf` to register.
pub static TMPFS_VFSCONF: VfsConf = VfsConf { name: "tmpfs", mount: tmpfs_mount };
