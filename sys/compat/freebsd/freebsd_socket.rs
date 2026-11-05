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


//! FreeBSD's socket calls, for the local domain (`AF_UNIX`): addresses
//! (`struct sockaddr_un`), messages (`struct msghdr`, `cmsghdr`,
//! `SCM_RIGHTS`), options, and process descriptors (`pdfork`). Layouts
//! and numbers from FreeBSD's headers; the work is the kernel's
//! (`kern::uipc_socket`, `kern::sys_procdesc`).

use framework::mm::{KArc, KVec, M_WAITOK};
use framework::user::{UserRegs, VmSpace};
use kern::kern_descrip::{File, FileFlags, FileKind};
use kern::kern_exec::vm_errno;
use kern::kern_malloc::M_TEMP;
use kern::kern_proc::Proc;
use kern::uipc_socket::{
    Domain, SUN_PATH, SendExtra, SockType, Socket, kern_accept, kern_connect, kern_listen, kern_recv, kern_send,
    kern_shutdown, kern_socketpair,
};
use kern::vfs_syscalls::{AtFd, kern_bind_path, kern_socket_node};
use libkern::conv::{trunc_i32, u64_from_usize, usize_from_u64};
use sys::errno::{Errno, KResult};

use crate::arch::cpu_set_syscall_retval;

const AF_UNIX: u8 = 1;
const SOCK_CLOEXEC: i32 = 0x1000_0000;
const SOCK_NONBLOCK: i32 = 0x2000_0000;
const SOL_SOCKET: i32 = 0xffff;
const SOL_LOCAL: i32 = 0;
const SCM_RIGHTS: i32 = 1;
const MSG_PEEK: i32 = 0x2;
const MSG_TRUNC: i32 = 0x10;
const MSG_CTRUNC: i32 = 0x20;
const MSG_DONTWAIT: i32 = 0x80;
const MSG_NOSIGNAL: i32 = 0x2_0000;
const MSG_CMSG_CLOEXEC: i32 = 0x4_0000;
/// `sizeof(struct sockaddr_un)`.
const SOCKADDR_UN: usize = 106;
/// Most bytes one call moves: a stream send takes the rest next time.
const MAX_IO: usize = kern::uipc_socket::SOCKBUF;

/// The socket open as `fd`, and how the file was opened.
fn sock(p: &Proc, fd: i32) -> KResult<(KArc<Socket>, FileFlags)> {
    let f = p.fd.get(fd)?;
    let flags = *f.flags.lock();
    match &f.kind {
        FileKind::Socket(s) => Ok((s.clone(), flags)),
        _ => Err(Errno::ENOTSOCK),
    }
}

fn ok(v: u64) -> KResult<[u64; 2]> {
    Ok([v, 0])
}

/// A new socket file in `p`'s table.
fn install(p: &Proc, s: KArc<Socket>, flags: i32) -> KResult<i32> {
    let ff = FileFlags { read: true, write: true, append: false, nonblock: flags & SOCK_NONBLOCK != 0 };
    let f = File::new(FileKind::Socket(s), ff)?;
    p.fd.alloc(f, 0, flags & SOCK_CLOEXEC != 0)
}

fn sock_type(t: i32) -> KResult<SockType> {
    Ok(match t & 0xffff {
        1 => SockType::Stream,
        2 => SockType::Dgram,
        5 => SockType::SeqPacket,
        _ => return Err(Errno::EPROTONOSUPPORT),
    })
}

/// `socket(domain, type, protocol)`: the local domain only for now.
pub(crate) fn sys_socket(p: &Proc, domain: i32, ty: i32, proto: i32) -> KResult<[u64; 2]> {
    if domain != i32::from(AF_UNIX) {
        return Err(Errno::EAFNOSUPPORT);
    }
    if proto != 0 {
        return Err(Errno::EPROTONOSUPPORT);
    }
    let s = Socket::new(Domain::Local, sock_type(ty)?, p)?;
    ok(u64::from(install(p, s, ty)?.unsigned_abs()))
}

/// `socketpair(domain, type, protocol, rsv)`.
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_socketpair(p: &Proc, domain: i32, ty: i32, proto: i32, rsv: u64) -> KResult<[u64; 2]> {
    if domain != i32::from(AF_UNIX) {
        return Err(Errno::EAFNOSUPPORT);
    }
    if proto != 0 {
        return Err(Errno::EPROTONOSUPPORT);
    }
    let kind = sock_type(ty)?;
    let (a, b) = (Socket::new(Domain::Local, kind, p)?, Socket::new(Domain::Local, kind, p)?);
    kern_socketpair(&a, &b);
    let fa = install(p, a, ty)?;
    let fb = match install(p, b, ty) {
        Ok(fb) => fb,
        Err(e) => {
            let _ = p.fd.close(fa);
            return Err(e);
        }
    };
    let mut out = [0u8; 8];
    out[..4].copy_from_slice(&fa.to_le_bytes());
    out[4..].copy_from_slice(&fb.to_le_bytes());
    if let Err(e) = p.vm().copyout(rsv, &out) {
        let _ = p.fd.close(fa);
        let _ = p.fd.close(fb);
        return Err(vm_errno(e));
    }
    ok(0)
}

/// The path of a `struct sockaddr_un` at `addr`, `len` bytes long.
fn sun_path(vm: &VmSpace, addr: u64, len: u64, out: &mut [u8; SUN_PATH]) -> KResult<usize> {
    let len = usize_from_u64(len);
    if !(3..=SOCKADDR_UN).contains(&len) {
        return Err(Errno::EINVAL);
    }
    let mut b = [0u8; SOCKADDR_UN];
    vm.copyin(addr, &mut b[..len]).map_err(vm_errno)?;
    if b[1] != AF_UNIX {
        return Err(Errno::EAFNOSUPPORT);
    }
    let path = &b[2..len];
    let n = path.iter().position(|&c| c == 0).unwrap_or(path.len());
    if n == 0 {
        return Err(Errno::EINVAL);
    }
    out[..n].copy_from_slice(&path[..n]);
    Ok(n)
}

/// Writes a `struct sockaddr_un` for `path` at `addr`, whose room is in
/// `*lenp` (updated to the full length), as `getsockname` and the others
/// do.
fn sun_out(vm: &VmSpace, path: &[u8], addr: u64, lenp: u64) -> KResult<()> {
    if addr == 0 || lenp == 0 {
        return Ok(());
    }
    let mut lb = [0u8; 4];
    vm.copyin(lenp, &mut lb).map_err(vm_errno)?;
    let room = usize::try_from(u32::from_le_bytes(lb)).unwrap_or(0);
    let mut b = [0u8; SOCKADDR_UN];
    let full = (2 + path.len() + 1).clamp(16, SOCKADDR_UN);
    b[0] = u8::try_from(full).unwrap_or(0);
    b[1] = AF_UNIX;
    let n = path.len().min(SUN_PATH);
    b[2..2 + n].copy_from_slice(&path[..n]);
    vm.copyout(addr, &b[..full.min(room)]).map_err(vm_errno)?;
    vm.copyout(lenp, &u32::try_from(full).unwrap_or(0).to_le_bytes()).map_err(vm_errno)?;
    Ok(())
}

/// `bind(s, name, namelen)`: a name in the file system.
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_bind(p: &Proc, fd: i32, name: u64, len: u64) -> KResult<[u64; 2]> {
    let (s, _) = sock(p, fd)?;
    let mut path = [0u8; SUN_PATH];
    let n = sun_path(&p.vm(), name, len, &mut path)?;
    kern_bind_path(p, AtFd::Cwd, &s, &path[..n])?;
    ok(0)
}

/// `listen(s, backlog)`.
pub(crate) fn sys_listen(p: &Proc, fd: i32, backlog: i32) -> KResult<[u64; 2]> {
    let (s, _) = sock(p, fd)?;
    kern_listen(&s, usize::try_from(backlog).unwrap_or(0))?;
    ok(0)
}

/// `connect(s, name, namelen)`.
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_connect(p: &Proc, fd: i32, name: u64, len: u64) -> KResult<[u64; 2]> {
    let (s, _) = sock(p, fd)?;
    let mut path = [0u8; SUN_PATH];
    let n = sun_path(&p.vm(), name, len, &mut path)?;
    let vp = kern_socket_node(p, AtFd::Cwd, &path[..n])?;
    kern_connect(p, &s, &vp)?;
    ok(0)
}

/// `accept(s, name, anamelen)` and `accept4(..., flags)`.
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_accept(p: &Proc, fd: i32, name: u64, lenp: u64, flags: i32) -> KResult<[u64; 2]> {
    let (s, ff) = sock(p, fd)?;
    let c = kern_accept(p, &s, ff.nonblock)?;
    let mut peer = [0u8; SUN_PATH];
    let n = c.peer_name(&mut peer).unwrap_or(0);
    let nfd = install(p, c, flags)?;
    sun_out(&p.vm(), &peer[..n], name, lenp)?;
    ok(u64::from(nfd.unsigned_abs()))
}

/// `shutdown(s, how)`.
pub(crate) fn sys_shutdown(p: &Proc, fd: i32, how: i32) -> KResult<[u64; 2]> {
    let (s, _) = sock(p, fd)?;
    let (rd, wr) = match how {
        0 => (true, false),
        1 => (false, true),
        2 => (true, true),
        _ => return Err(Errno::EINVAL),
    };
    kern_shutdown(&s, rd, wr)?;
    ok(0)
}

/// Sends `data` with descriptors `rights`, to `to` (an address) if given.
fn send_common(p: &Proc, fd: i32, data: &[u8], rights: KVec<KArc<File>>, to: Option<(u64, u64)>, flags: i32) -> KResult<[u64; 2]> {
    let (s, ff) = sock(p, fd)?;
    let mut path = [0u8; SUN_PATH];
    let target = match to {
        Some((addr, len)) if addr != 0 => {
            let n = sun_path(&p.vm(), addr, len, &mut path)?;
            Some(kern_socket_node(p, AtFd::Cwd, &path[..n])?)
        }
        _ => None,
    };
    let extra = SendExtra {
        rights,
        to: target.as_ref(),
        nosignal: flags & MSG_NOSIGNAL != 0,
        nonblock: ff.nonblock || flags & MSG_DONTWAIT != 0,
    };
    ok(u64_from_usize(kern_send(p, &s, data, extra)?))
}

/// A kernel buffer of `len` bytes (at most [`MAX_IO`]).
fn kbuf(len: usize) -> KResult<KVec<u8>> {
    let mut v = KVec::with_capacity(len, &M_TEMP, M_WAITOK)?;
    for _ in 0..len {
        v.push(0, M_WAITOK)?;
    }
    Ok(v)
}

/// `sendto(s, buf, len, flags, to, tolen)`.
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_sendto(p: &Proc, args: &[u64; 6]) -> KResult<[u64; 2]> {
    let len = usize_from_u64(args[2]).min(MAX_IO);
    let mut data = kbuf(len)?;
    p.vm().copyin(args[1], &mut data).map_err(vm_errno)?;
    send_common(p, trunc_i32(args[0]), &data, KVec::new(&M_TEMP), Some((args[4], args[5])), trunc_i32(args[3]))
}

/// `recvfrom(s, buf, len, flags, from, fromlenaddr)`.
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_recvfrom(p: &Proc, args: &[u64; 6]) -> KResult<[u64; 2]> {
    let fd = trunc_i32(args[0]);
    let flags = trunc_i32(args[3]);
    let (s, ff) = sock(p, fd)?;
    let mut data = kbuf(usize_from_u64(args[2]).min(MAX_IO))?;
    let r = kern_recv(p, &s, &mut data, flags & MSG_PEEK != 0, ff.nonblock || flags & MSG_DONTWAIT != 0)?;
    let vm = p.vm();
    vm.copyout(args[1], data.get(..r.len).unwrap_or(&[])).map_err(vm_errno)?;
    sun_out(&vm, &r.from, args[4], args[5])?;
    ok(u64_from_usize(r.len))
}

/// `struct msghdr`, as read.
struct MsgHdr {
    name: u64,
    namelen: u32,
    iov: u64,
    iovlen: u32,
    control: u64,
    controllen: u32,
}

fn msghdr_in(vm: &VmSpace, msg: u64) -> KResult<MsgHdr> {
    let mut b = [0u8; 48];
    vm.copyin(msg, &mut b).map_err(vm_errno)?;
    let q = |o: usize| u64::from_le_bytes(b[o..o + 8].try_into().unwrap_or([0; 8]));
    let d = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap_or([0; 4]));
    Ok(MsgHdr { name: q(0), namelen: d(8), iov: q(16), iovlen: d(24), control: q(32), controllen: d(40) })
}

/// The pieces of an I/O vector, as (address, length).
fn iovecs(vm: &VmSpace, iov: u64, n: u32) -> KResult<KVec<(u64, usize)>> {
    if n > 1024 {
        return Err(Errno::EMSGSIZE);
    }
    let mut v = KVec::with_capacity(usize::try_from(n).unwrap_or(0), &M_TEMP, M_WAITOK)?;
    for i in 0..u64::from(n) {
        let mut b = [0u8; 16];
        vm.copyin(iov + 16 * i, &mut b).map_err(vm_errno)?;
        let base = u64::from_le_bytes(b[..8].try_into().map_err(|_| Errno::EFAULT)?);
        let len = usize_from_u64(u64::from_le_bytes(b[8..].try_into().map_err(|_| Errno::EFAULT)?));
        v.push((base, len), M_WAITOK)?;
    }
    Ok(v)
}

/// `sendmsg(s, msg, flags)`: the pieces gathered, `SCM_RIGHTS`
/// descriptors passed.
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_sendmsg(p: &Proc, fd: i32, msg: u64, flags: i32) -> KResult<[u64; 2]> {
    let vm = p.vm();
    let m = msghdr_in(&vm, msg)?;
    let iov = iovecs(&vm, m.iov, m.iovlen)?;
    let total = iov.iter().map(|v| v.1).sum::<usize>().min(MAX_IO);
    let mut data = kbuf(total)?;
    let mut at = 0;
    for &(base, len) in iov.iter() {
        let n = len.min(total - at);
        vm.copyin(base, data.get_mut(at..at + n).ok_or(Errno::EFAULT)?).map_err(vm_errno)?;
        at += n;
    }
    // Control messages: `cmsg_len`, `cmsg_level`, `cmsg_type`, then the
    // data at 16, each padded to 8 bytes.
    let mut rights: KVec<KArc<File>> = KVec::new(&M_TEMP);
    let clen = usize::try_from(m.controllen).unwrap_or(0).min(4096);
    let mut off = 0;
    while m.control != 0 && off + 16 <= clen {
        let mut h = [0u8; 12];
        vm.copyin(m.control + u64_from_usize(off), &mut h).map_err(vm_errno)?;
        let len = usize::try_from(u32::from_le_bytes([h[0], h[1], h[2], h[3]])).unwrap_or(0);
        let level = i32::from_le_bytes([h[4], h[5], h[6], h[7]]);
        let ty = i32::from_le_bytes([h[8], h[9], h[10], h[11]]);
        if len < 16 || off + len > clen {
            return Err(Errno::EINVAL);
        }
        if level == SOL_SOCKET && ty == SCM_RIGHTS {
            for k in 0..(len - 16) / 4 {
                let mut b = [0u8; 4];
                vm.copyin(m.control + u64_from_usize(off + 16 + 4 * k), &mut b).map_err(vm_errno)?;
                rights.push(p.fd.get(i32::from_le_bytes(b))?, M_WAITOK)?;
            }
        }
        off += len.next_multiple_of(8);
    }
    let to = (m.name != 0).then_some((m.name, u64::from(m.namelen)));
    send_common(p, fd, &data, rights, to, flags)
}

/// `recvmsg(s, msg, flags)`: the pieces scattered, descriptors received
/// installed (as many as the control buffer holds; `MSG_CTRUNC`
/// otherwise).
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_recvmsg(p: &Proc, fd: i32, msg: u64, flags: i32) -> KResult<[u64; 2]> {
    let (s, ff) = sock(p, fd)?;
    let vm = p.vm();
    let m = msghdr_in(&vm, msg)?;
    let iov = iovecs(&vm, m.iov, m.iovlen)?;
    let total = iov.iter().map(|v| v.1).sum::<usize>().min(MAX_IO);
    let mut data = kbuf(total)?;
    let r = kern_recv(p, &s, &mut data, flags & MSG_PEEK != 0, ff.nonblock || flags & MSG_DONTWAIT != 0)?;
    let mut at = 0;
    for &(base, len) in iov.iter() {
        if at >= r.len {
            break;
        }
        let n = len.min(r.len - at);
        vm.copyout(base, data.get(at..at + n).ok_or(Errno::EFAULT)?).map_err(vm_errno)?;
        at += n;
    }
    let mut out_flags = if r.truncated { MSG_TRUNC } else { 0 };
    // The descriptors, as one SCM_RIGHTS message.
    let mut clen = 0usize;
    if !r.rights.is_empty() {
        let room = usize::try_from(m.controllen).unwrap_or(0);
        let fit = if m.control == 0 || room < 16 { 0 } else { (room - 16) / 4 };
        let n = r.rights.len().min(fit);
        if n < r.rights.len() {
            out_flags |= MSG_CTRUNC;
        }
        if n > 0 {
            let cloexec = flags & MSG_CMSG_CLOEXEC != 0;
            let mut fds: KVec<i32> = KVec::with_capacity(n, &M_TEMP, M_WAITOK)?;
            for f in r.rights.iter().take(n) {
                fds.push(p.fd.alloc(f.clone(), 0, cloexec)?, M_WAITOK)?;
            }
            let len = 16 + 4 * n;
            let mut h = [0u8; 16];
            h[..4].copy_from_slice(&u32::try_from(len).unwrap_or(0).to_le_bytes());
            h[4..8].copy_from_slice(&SOL_SOCKET.to_le_bytes());
            h[8..12].copy_from_slice(&SCM_RIGHTS.to_le_bytes());
            vm.copyout(m.control, &h).map_err(vm_errno)?;
            for (k, fdn) in fds.iter().enumerate() {
                vm.copyout(m.control + u64_from_usize(16 + 4 * k), &fdn.to_le_bytes()).map_err(vm_errno)?;
            }
            clen = len.next_multiple_of(8).min(room);
        }
    }
    vm.copyout(msg + 40, &u32::try_from(clen).unwrap_or(0).to_le_bytes()).map_err(vm_errno)?;
    vm.copyout(msg + 44, &out_flags.to_le_bytes()).map_err(vm_errno)?;
    if m.name != 0 {
        let mut b = [0u8; SOCKADDR_UN];
        let full = (2 + r.from.len() + 1).clamp(16, SOCKADDR_UN);
        b[0] = u8::try_from(full).unwrap_or(0);
        b[1] = AF_UNIX;
        let n = r.from.len().min(SUN_PATH);
        b[2..2 + n].copy_from_slice(r.from.get(..n).unwrap_or(&[]));
        let room = usize::try_from(m.namelen).unwrap_or(0).min(full);
        vm.copyout(m.name, &b[..room]).map_err(vm_errno)?;
        vm.copyout(msg + 8, &u32::try_from(room).unwrap_or(0).to_le_bytes()).map_err(vm_errno)?;
    }
    ok(u64_from_usize(r.len))
}

/// `getsockname(s, asa, alen)` and `getpeername` (`peer`).
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_getname(p: &Proc, fd: i32, addr: u64, lenp: u64, peer: bool) -> KResult<[u64; 2]> {
    let (s, _) = sock(p, fd)?;
    let mut name = [0u8; SUN_PATH];
    let n = if peer { s.peer_name(&mut name)? } else { s.name(&mut name) };
    sun_out(&p.vm(), &name[..n], addr, lenp)?;
    ok(0)
}

/// `getsockopt(s, level, name, val, avalsize)`: what programs ask of a
/// local socket.
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_getsockopt(p: &Proc, args: &[u64; 6]) -> KResult<[u64; 2]> {
    const SO_ACCEPTCONN: i32 = 0x2;
    const SO_SNDBUF: i32 = 0x1001;
    const SO_RCVBUF: i32 = 0x1002;
    const SO_ERROR: i32 = 0x1007;
    const SO_TYPE: i32 = 0x1008;
    const SO_PROTOCOL: i32 = 0x1016;
    const SO_DOMAIN: i32 = 0x1019;
    const LOCAL_PEERCRED: i32 = 1;
    let (s, _) = sock(p, trunc_i32(args[0]))?;
    let (level, name, val, lenp) = (trunc_i32(args[1]), trunc_i32(args[2]), args[3], args[4]);
    let mut out = [0u8; 88];
    let len = match (level, name) {
        (SOL_SOCKET, SO_TYPE) => {
            let t: i32 = match s.kind {
                SockType::Stream => 1,
                SockType::Dgram => 2,
                SockType::SeqPacket => 5,
            };
            out[..4].copy_from_slice(&t.to_le_bytes());
            4
        }
        (SOL_SOCKET, SO_ERROR | SO_PROTOCOL) => 4,
        (SOL_SOCKET, SO_DOMAIN) => {
            out[..4].copy_from_slice(&1i32.to_le_bytes());
            4
        }
        (SOL_SOCKET, SO_ACCEPTCONN) => {
            out[..4].copy_from_slice(&i32::from(s.listening()).to_le_bytes());
            4
        }
        (SOL_SOCKET, SO_SNDBUF | SO_RCVBUF) => {
            out[..4].copy_from_slice(&i32::try_from(kern::uipc_socket::SOCKBUF).unwrap_or(0).to_le_bytes());
            4
        }
        // struct xucred: version, uid, ngroups, groups[16], pid.
        (SOL_LOCAL, LOCAL_PEERCRED) => {
            let (c, pid) = s.peer_cred().ok_or(Errno::ENOTCONN)?;
            out[4..8].copy_from_slice(&c.euid.to_le_bytes());
            let g = c.groups();
            let n = (g.len() + 1).min(16);
            out[8..10].copy_from_slice(&i16::try_from(n).unwrap_or(0).to_le_bytes());
            out[12..16].copy_from_slice(&c.egid.to_le_bytes());
            for (k, gid) in g.iter().take(n - 1).enumerate() {
                out[16 + 4 * k..20 + 4 * k].copy_from_slice(&gid.to_le_bytes());
            }
            out[0x50..0x54].copy_from_slice(&pid.to_le_bytes());
            0x58
        }
        _ => return Err(Errno::ENOPROTOOPT),
    };
    let vm = p.vm();
    let mut lb = [0u8; 4];
    vm.copyin(lenp, &mut lb).map_err(vm_errno)?;
    let room = usize::try_from(u32::from_le_bytes(lb)).unwrap_or(0);
    let n = len.min(room);
    vm.copyout(val, &out[..n]).map_err(vm_errno)?;
    vm.copyout(lenp, &u32::try_from(n).unwrap_or(0).to_le_bytes()).map_err(vm_errno)?;
    ok(0)
}

/// `setsockopt(s, level, name, val, valsize)`: accepted for the local
/// domain's options, which change nothing yet (buffer sizes, timeouts,
/// `SO_NOSIGPIPE`, `LOCAL_CREDS`). TODO(socket).
pub(crate) fn sys_setsockopt(p: &Proc, fd: i32) -> KResult<[u64; 2]> {
    sock(p, fd)?;
    ok(0)
}

/// `pdfork(fdp, flags)`: a child the caller holds by a descriptor; the
/// child returns 0, the parent its pid.
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_pdfork(p: &Proc, regs: &UserRegs, fdp: u64, flags: i32) -> KResult<[u64; 2]> {
    const PD_DAEMON: i32 = 1;
    const PD_CLOEXEC: i32 = 2;
    let mut child = *regs;
    cpu_set_syscall_retval(&mut child, Ok([0, 1]));
    let (pid, fd) = kern::kern_fork::kern_pdfork(p, child, flags & PD_DAEMON != 0, flags & PD_CLOEXEC != 0)?;
    p.vm().copyout(fdp, &fd.to_le_bytes()).map_err(vm_errno)?;
    ok(u64::from(pid))
}

fn procdesc(p: &Proc, fd: i32) -> KResult<KArc<kern::sys_procdesc::ProcDesc>> {
    match &p.fd.get(fd)?.kind {
        FileKind::ProcDesc(pd) => Ok(pd.clone()),
        _ => Err(Errno::EINVAL),
    }
}

/// `pdgetpid(fd, pidp)`.
pub(crate) fn sys_pdgetpid(p: &Proc, fd: i32, pidp: u64) -> KResult<[u64; 2]> {
    let pd = procdesc(p, fd)?;
    p.vm().copyout(pidp, &pd.pid.to_le_bytes()).map_err(vm_errno)?;
    ok(0)
}

/// `pdkill(fd, signum)`.
pub(crate) fn sys_pdkill(p: &Proc, fd: i32, sig: i32) -> KResult<[u64; 2]> {
    let pd = procdesc(p, fd)?;
    let sig = crate::freebsd_signal::from_signo(sig).ok_or(Errno::EINVAL)?;
    kern::kern_sig::kern_kill(p, kern::kern_sig::KillWho::Pid(pd.pid), Some(sig))?;
    ok(0)
}
