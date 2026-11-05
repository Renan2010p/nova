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


//! The page cache (FreeBSD's `vm/vnode_pager.c` and the vnode's
//! `vm_object`): pages of files, read once and shared by every mapping of
//! them (programs' text, libraries, `mmap`). A mapping that writes gets
//! its own copy (copy-on-write, in the framework).
//!
//! An object is a file (a vnode), known to the framework by a number. A
//! page stays cached while mapped or until its file is written,
//! truncated or removed.
//!
//! Lock order: a fault holds the address space's map, then takes the
//! cache, then reads the file (its file system's lock): nothing may take
//! the address space's map while holding a file system's lock (see
//! LOCKING.md).
//!
//! TODO(vm): give cached pages back when memory runs low, and pages read
//! ahead; map the memory disk's own pages instead of copying them.

use framework::mm::{Frame, KArc, KVec, M_WAITOK, MallocType};
use framework::sync::Mutex;
use framework::user::{Backing, Pager};
use framework::PAGE_SIZE;
use libkern::conv::{u64_from_usize, usize_from_u64};
use sys::errno::KResult;

use crate::vnode::Vnode;

/// The page cache (`M_VMPGDATA`).
static M_PAGECACHE: MallocType = MallocType::new("pagecache");

/// Files mapped, by object number.
static OBJECTS: Mutex<KVec<Vnode>> = Mutex::new(KVec::new(&M_PAGECACHE));
/// A cached page: (object, page number) and its memory.
type CachedPage = ((u64, u64), KArc<Frame>);

/// Cached pages, sorted by (object, page).
static CACHE: Mutex<KVec<CachedPage>> = Mutex::new(KVec::new(&M_PAGECACHE));

/// The pager over files.
pub struct VnodePager;

/// The instance regions point to.
pub static VNODE_PAGER: VnodePager = VnodePager;

fn object(id: u64) -> Option<Vnode> {
    OBJECTS.lock().get(usize_from_u64(id)).copied()
}

impl Pager for VnodePager {
    fn page(&self, id: u64, index: u64) -> Option<KArc<Frame>> {
        let key = (id, index);
        let mut cache = CACHE.lock();
        let at = match cache.binary_search_by_key(&key, |e| e.0) {
            Ok(i) => return cache.get(i).map(|e| e.1.clone()),
            Err(at) => at,
        };
        let vp = object(id)?;
        let mut f = Frame::alloc()?;
        let mut buf = [0u8; 512];
        let base = index * u64_from_usize(PAGE_SIZE);
        for off in (0..PAGE_SIZE).step_by(buf.len()) {
            let n = vp.read(base + u64_from_usize(off), &mut buf).ok()?;
            buf[n..].fill(0);
            f.write_bytes(off, &buf);
        }
        let f = KArc::new(f, &M_PAGECACHE, M_WAITOK).ok()?;
        cache.insert(at, (key, f.clone()), M_WAITOK).ok()?;
        Some(f)
    }

    fn read(&self, id: u64, off: u64, buf: &mut [u8]) -> bool {
        let Some(vp) = object(id) else { return false };
        let mut done = 0;
        while done < buf.len() {
            match vp.read(off + u64_from_usize(done), buf.get_mut(done..).unwrap_or(&mut [])) {
                Ok(0) => break,
                Ok(n) => done += n,
                Err(_) => return false,
            }
        }
        true
    }
}

/// The object number of `vp`, registering it the first time.
fn object_id(vp: &Vnode) -> KResult<u64> {
    let mut objs = OBJECTS.lock();
    if let Some(i) = objs.iter().position(|o| o.same(vp)) {
        return Ok(u64_from_usize(i));
    }
    objs.push(*vp, M_WAITOK)?;
    Ok(u64_from_usize(objs.len() - 1))
}

/// A region's backing by `vp`, from `offset`, showing `len` bytes of it
/// (`vnode_pager_alloc`).
pub fn vnode_backing(vp: &Vnode, offset: u64, len: u64) -> KResult<Backing> {
    Ok(Backing { pager: &VNODE_PAGER, id: object_id(vp)?, offset, len })
}

/// Forgets the cached pages of `vp`: it was written, truncated or
/// removed (`vnode_pager_setsize`, `vm_object_page_remove`). Mappings
/// keep the pages they have.
pub fn vnode_pager_invalidate(vp: &Vnode) {
    let id = {
        let objs = OBJECTS.lock();
        match objs.iter().position(|o| o.same(vp)) {
            Some(i) => u64_from_usize(i),
            None => return,
        }
    };
    let mut cache = CACHE.lock();
    let first = cache.partition_point(|e| e.0.0 < id);
    while cache.get(first).is_some_and(|e| e.0.0 == id) {
        drop(cache.remove(first));
    }
}
