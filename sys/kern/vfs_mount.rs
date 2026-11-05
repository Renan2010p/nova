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


//! Mounting (FreeBSD's `kern/vfs_mount.c`, `vfs_mountroot.c`, and the
//! memory disk of `dev/md/md.c`).
//!
//! File systems register themselves by name ([`vfs_register`], from
//! `sys/conf`). The root is mounted from `vfs.root.mountfrom`
//! (`<type>:<device>`); with none set, every registered type is tried on
//! the memory disk, as FreeBSD's `MD_ROOT` does. The only device is the
//! memory disk `md0`: an image the loader preloaded (`md_image` or
//! `mfs_root`, FreeBSD's `mfsroot_load="YES"`).

use framework::BootInfo;
use framework::sync::{MtxSpin, OnceLock};
use sys::errno::{Errno, KResult};

use crate::kern_environment::StaticEnv;
use crate::printf;
use crate::vnode::{Mount, Vnode, VfsOps};

/// A file system type (`struct vfsconf`).
pub struct VfsConf {
    /// Its name, as in `vfs.root.mountfrom` (`vfc_name`).
    pub name: &'static str,
    /// Mounts it on a memory image (`vfs_mount`).
    pub mount: fn(&'static [u8]) -> KResult<&'static dyn VfsOps>,
}

/// Most file system types a kernel is built with.
pub const MAX_VFS: usize = 8;

static VFSCONF: MtxSpin<[Option<&'static VfsConf>; MAX_VFS]> = MtxSpin::new([None; MAX_VFS]);
static ROOT: OnceLock<Mount> = OnceLock::new();

/// Adds a file system type (`vfs_register`). Called by the kernel binary
/// before `mi_startup`.
///
/// # Panics
///
/// If [`MAX_VFS`] types are registered already.
pub fn vfs_register(vfc: &'static VfsConf) {
    let mut table = VFSCONF.lock();
    let Some(slot) = table.iter_mut().find(|s| s.is_none()) else {
        panic!("vfs_register: more than {MAX_VFS} file systems");
    };
    *slot = Some(vfc);
}

/// The root directory (`rootvnode`), once mounted.
pub fn rootvnode() -> Option<Vnode> {
    ROOT.get().map(|mp| Vnode { mp, ino: mp.ops.root() })
}

/// The memory disk's device name.
const MD0: &str = "/dev/md0";

/// The image preloaded for the memory disk, announced as `md(4)` does.
fn md_preloaded(boot: &BootInfo<'_>) -> Option<&'static [u8]> {
    let m = boot.preload.modules().find(|m| matches!(m.type_name(), Some("md_image" | "mfs_root")))?;
    let image = crate::kern_linker::preload_module(boot, &m)?;
    printf!("md0: Preloaded image <{}> {} bytes at {:#x}\n", m.name().unwrap_or("?"), image.len(), image.as_ptr().addr());
    Some(image)
}

/// Mounts the root file system (`vfs_mountroot`). Without one, the system
/// goes on with what the loader preloaded.
pub fn vfs_mountroot(boot: &BootInfo<'_>, kenv: &StaticEnv<'_>) {
    let Some(image) = md_preloaded(boot) else { return };
    let mut tried = false;
    let types = VFSCONF.lock();
    let wanted = kenv.getenv("vfs.root.mountfrom");
    for spec in wanted.unwrap_or("").split(';').filter(|s| !s.is_empty()) {
        let (name, dev) = spec.split_once(':').unwrap_or((spec, ""));
        tried = true;
        printf!("Trying to mount root from {}:{} []...\n", name, dev);
        let vfc = types.iter().flatten().find(|v| v.name == name);
        let result = match vfc {
            Some(vfc) if dev == MD0 => mount_root(vfc, image),
            Some(_) => Err(Errno::ENOENT),
            None => Err(Errno::ENOSYS),
        };
        match result {
            Ok(()) => return,
            Err(e) => printf!("Mounting from {}:{} failed with error {:?}.\n", name, dev, e),
        }
    }
    if tried {
        return;
    }
    // No `vfs.root.mountfrom`: every type, on the memory disk.
    for vfc in types.iter().flatten() {
        printf!("Trying to mount root from {}:{} []...\n", vfc.name, MD0);
        if mount_root(vfc, image).is_ok() {
            return;
        }
    }
}

fn mount_root(vfc: &'static VfsConf, image: &'static [u8]) -> KResult<()> {
    let ops = (vfc.mount)(image)?;
    ROOT.set(Mount { ops, fstype: vfc.name, from: MD0, on: "/", rdonly: true }).map_err(|_| Errno::EBUSY)
}
