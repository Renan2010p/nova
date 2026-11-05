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


//! Path name lookup (FreeBSD's `kern/vfs_lookup.c`, `namei`).

use framework::mm::{KVec, M_WAITOK};
use sys::errno::{Errno, KResult};

use crate::kern_malloc::M_TEMP;
use crate::vnode::{VType, Vnode};

/// Longest path (`MAXPATHLEN`, with its NUL).
pub const MAXPATHLEN: usize = 1024;
/// Most symbolic links followed in one lookup (`MAXSYMLINKS`).
pub const MAXSYMLINKS: u32 = 32;
/// Longest name of a path component (`NAME_MAX`).
pub const NAME_MAX: usize = 255;

/// Looks `path` up (`namei`): from the root if it starts with `/`, from
/// `dir` otherwise. The last component is followed if it is a symbolic
/// link and `follow` (`FOLLOW`/`NOFOLLOW`).
pub fn namei(root: Vnode, dir: Vnode, path: &[u8], follow: bool) -> KResult<Vnode> {
    if path.is_empty() {
        return Err(Errno::ENOENT);
    }
    if path.len() >= MAXPATHLEN {
        return Err(Errno::ENAMETOOLONG);
    }
    // The path still to walk: rewritten when a link is followed.
    let mut rest: KVec<u8> = KVec::with_capacity(MAXPATHLEN, &M_TEMP, M_WAITOK)?;
    rest.extend_from_slice(path, M_WAITOK)?;
    let mut cur = dir;
    let mut links = 0;
    let mut at = 0;
    loop {
        // An absolute path, or link target, starts again at the root.
        if at == 0 && rest.first() == Some(&b'/') {
            cur = root;
        }
        // The next component.
        while rest.get(at) == Some(&b'/') {
            at += 1;
        }
        if at >= rest.len() {
            return Ok(cur);
        }
        let end = rest.get(at..).and_then(|r| r.iter().position(|&c| c == b'/')).map_or(rest.len(), |n| at + n);
        let name = rest.get(at..end).ok_or(Errno::EINVAL)?;
        if name.len() > NAME_MAX {
            return Err(Errno::ENAMETOOLONG);
        }
        let last = rest.get(end..).is_none_or(|r| r.iter().all(|&c| c == b'/'));
        if cur.getattr()?.kind != VType::Dir {
            return Err(Errno::ENOTDIR);
        }
        let next = if name == b"." || (name == b".." && cur.same(&root)) {
            cur
        } else if name == b".." {
            // Out of a mount's root, through the directory it covers.
            let mut base = cur;
            while let Some(c) = crate::vfs_mount::covered(&base) {
                base = c;
            }
            if base.same(&root) { root } else { Vnode { mp: base.mp, ino: base.mp.ops.lookup(base.ino, name)? } }
        } else {
            let mut v = Vnode { mp: cur.mp, ino: cur.mp.ops.lookup(cur.ino, name)? };
            // Into what is mounted on it (`v_mountedhere`).
            while let Some(r) = crate::vfs_mount::mounted_here(&v) {
                v = r;
            }
            v
        };
        let attr = next.getattr()?;
        // A trailing slash asks for a directory, so it follows a link too.
        let trailing = rest.len() > end;
        if attr.kind == VType::Lnk && (!last || follow || trailing) {
            links += 1;
            if links > MAXSYMLINKS {
                return Err(Errno::ELOOP);
            }
            let mut target = [0u8; MAXPATHLEN];
            let n = next.mp.ops.readlink(next.ino, &mut target)?;
            let target = target.get(..n).ok_or(Errno::EIO)?;
            if target.is_empty() {
                return Err(Errno::ENOENT);
            }
            // The link's target, then what followed the link.
            let tail_len = rest.len() - end;
            if n + tail_len >= MAXPATHLEN {
                return Err(Errno::ENAMETOOLONG);
            }
            let mut new: KVec<u8> = KVec::with_capacity(MAXPATHLEN, &M_TEMP, M_WAITOK)?;
            new.extend_from_slice(target, M_WAITOK)?;
            new.extend_from_slice(rest.get(end..).unwrap_or(&[]), M_WAITOK)?;
            rest = new;
            at = 0;
            continue; // `cur` stays the link's directory
        }
        cur = next;
        at = end;
    }
}

/// Looks up the directory that holds the last component of `path`
/// (`namei` with `LOCKPARENT`, for `CREATE`, `DELETE`, `RENAME`): the
/// directory and that name. Trailing slashes are dropped; `EINVAL` for
/// `.` or `..` as the name, `ENOENT` for an empty path.
pub fn namei_parent(root: Vnode, dir: Vnode, path: &[u8]) -> KResult<(Vnode, &[u8])> {
    let mut end = path.len();
    while end > 1 && path.get(end - 1) == Some(&b'/') {
        end -= 1;
    }
    let path = path.get(..end).ok_or(Errno::ENOENT)?;
    if path.is_empty() {
        return Err(Errno::ENOENT);
    }
    let (dirpart, name) = match path.iter().rposition(|&c| c == b'/') {
        Some(0) => (&b"/"[..], path.get(1..).unwrap_or(&[])),
        Some(i) => (path.get(..i).unwrap_or(&[]), path.get(i + 1..).unwrap_or(&[])),
        None => (&b"."[..], path),
    };
    if name.is_empty() || name == b"." || name == b".." {
        return Err(Errno::EINVAL);
    }
    if name.len() > NAME_MAX {
        return Err(Errno::ENAMETOOLONG);
    }
    let parent = namei(root, dir, dirpart, true)?;
    if parent.getattr()?.kind != VType::Dir {
        return Err(Errno::ENOTDIR);
    }
    Ok((parent, name))
}
