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

//! FreeBSD's `sysctl(3)` (`kern/kern_sysctl.c`, `kern/kern_mib.c`): the
//! numbers and dotted names programs use, and the C types of the values,
//! for what the kernel knows.
//!
//! Like FreeBSD's Linux ABI answering "Linux", this personality answers
//! as the system it is compatible with: `kern.ostype` is "FreeBSD" and
//! `kern.osrelease` the release whose ABI it implements; `kern.version`
//! is Nova's own banner.
//!
//! A name not in the table is `ENOENT`, as FreeBSD answers; a verbose boot
//! shows which ones programs ask for.

use kern::kern_exec::vm_errno;
use kern::kern_mib::{FixedBuf, MAXHOSTNAMELEN, hostname, set_hostname, version};
use kern::kern_proc::Proc;
use kern::{kern_random, printf};
use libkern::conv::{trunc_u32, u64_from_usize, usize_from_u64};
use sys::errno::{Errno, KResult};

use crate::FREEBSD_VERSION;
use crate::arch::{PAGESIZES, USRSTACK};

/// `CTL_MAXNAME`.
const CTL_MAXNAME: usize = 24;

/// What a node holds.
#[derive(Clone, Copy)]
enum Value {
    /// A C `int`.
    Int(fn() -> i32),
    /// A C `u_long`/`size_t`.
    Ulong(fn() -> u64),
    /// A string, returned with its NUL.
    Str(fn(&mut FixedBuf<256>)),
    /// `kern.hostname`: a string root may set.
    Hostname,
    /// `kern.arandom`: random bytes, as many as asked (at most 256).
    Arandom,
    /// A C structure, with its `sysctl(8)` format (`S,timeval`): the bytes
    /// written, and their count.
    Struct(fn(&mut [u8; 64]) -> usize, &'static str),
}

impl Value {
    /// `oid_kind` (type and flags) and `oid_fmt`, as `sysctl(8)` asks
    /// with `{0, 4, ...}`.
    fn kind_fmt(self) -> (u32, &'static str) {
        const CTLTYPE_INT: u32 = 2;
        const CTLTYPE_STRING: u32 = 3;
        const CTLTYPE_OPAQUE: u32 = 5;
        const CTLTYPE_ULONG: u32 = 8;
        const RD: u32 = 0x8000_0000;
        const WR: u32 = 0x4000_0000;
        const MPSAFE: u32 = 0x0004_0000;
        match self {
            Value::Int(_) => (CTLTYPE_INT | RD | MPSAFE, "I"),
            Value::Ulong(_) => (CTLTYPE_ULONG | RD | MPSAFE, "LU"),
            Value::Str(_) => (CTLTYPE_STRING | RD | MPSAFE, "A"),
            Value::Hostname => (CTLTYPE_STRING | RD | WR | MPSAFE, "A"),
            Value::Arandom => (CTLTYPE_OPAQUE | RD | MPSAFE, ""),
            Value::Struct(_, fmt) => (CTLTYPE_OPAQUE | RD | MPSAFE, fmt),
        }
    }
}

/// One node: its numbers, its name, its value.
struct Node {
    mib: &'static [i32],
    name: &'static str,
    value: Value,
}

/// Memory not wired by the kernel (`hw.usermem`): what is free.
fn usermem() -> u64 {
    framework::mm::vm_phys::free_count() * libkern::conv::u64_from_usize(framework::PAGE_SIZE)
}

/// Pages of memory (`hw.availpages`, for `sysconf(_SC_PHYS_PAGES)`).
fn availpages() -> u64 {
    kern::kern_mib::physmem() / libkern::conv::u64_from_usize(framework::PAGE_SIZE)
}

fn ncpu() -> i32 {
    i32::try_from(framework::cpu::mp_ncpus()).unwrap_or(i32::MAX)
}

/// The FreeBSD sysctls Nova knows (numbers from `sys/sysctl.h`).
static NODES: &[Node] = &[
    Node { mib: &[1, 1], name: "kern.ostype", value: Value::Str(|b| b.buf_str("FreeBSD")) },
    Node { mib: &[1, 2], name: "kern.osrelease", value: Value::Str(|b| b.buf_str("15.1-RELEASE")) },
    Node { mib: &[1, 3], name: "kern.osrevision", value: Value::Int(|| 199_506) },
    Node { mib: &[1, 4], name: "kern.version", value: Value::Str(|b| { let _ = version(b); }) },
    Node { mib: &[1, 8], name: "kern.argmax", value: Value::Int(|| 524_288) },
    Node { mib: &[1, 10], name: "kern.hostname", value: Value::Hostname },
    Node { mib: &[1, 18], name: "kern.ngroups", value: Value::Int(|| 1023) },
    Node { mib: &[1, 24], name: "kern.osreldate", value: Value::Int(|| i32::try_from(FREEBSD_VERSION).unwrap_or(0)) },
    Node { mib: &[1, 9], name: "kern.securelevel", value: Value::Int(|| -1) },
    Node { mib: &[1, 21], name: "kern.boottime", value: Value::Struct(boottime, "S,timeval") },
    Node { mib: &[1, 26], name: "kern.bootfile", value: Value::Str(|b| b.buf_str("/boot/kernel/kernel")) },
    Node { mib: &[1, 33], name: "kern.usrstack", value: Value::Ulong(|| USRSTACK) },
    // OID_AUTO nodes: numbers from FreeBSD's dynamic range, stable here.
    Node { mib: &[1, 0x1_0001], name: "kern.pid_max", value: Value::Int(|| 99_999) },
    // `exp(-1/20)` in `FSCALE` fixed point: how fast %CPU decays.
    Node { mib: &[1, 0x1_0002], name: "kern.ccpu", value: Value::Int(|| 1948) },
    // `FSCALE`: the scale of fixed-point load and CPU figures.
    Node { mib: &[1, 0x1_0003], name: "kern.fscale", value: Value::Int(|| 2048) },
    Node { mib: &[1, 37], name: "kern.arandom", value: Value::Arandom },
    Node { mib: &[2, 2], name: "vm.loadavg", value: Value::Struct(loadavg, "S,loadavg") },
    Node { mib: &[6, 1], name: "hw.machine", value: Value::Str(|b| b.buf_str(framework::MACHINE)) },
    Node { mib: &[6, 3], name: "hw.ncpu", value: Value::Int(ncpu) },
    Node { mib: &[6, 4], name: "hw.byteorder", value: Value::Int(|| 1234) },
    Node { mib: &[6, 5], name: "hw.physmem", value: Value::Ulong(kern::kern_mib::physmem) },
    Node { mib: &[6, 6], name: "hw.usermem", value: Value::Ulong(usermem) },
    // OID_AUTO nodes: numbers from FreeBSD's dynamic range, stable here.
    Node { mib: &[6, 0x1_0001], name: "hw.realmem", value: Value::Ulong(kern::kern_mib::realmem) },
    Node { mib: &[6, 0x1_0002], name: "hw.availpages", value: Value::Ulong(availpages) },
    Node { mib: &[6, 7], name: "hw.pagesize", value: Value::Int(|| i32::try_from(framework::PAGE_SIZE).unwrap_or(4096)) },
    Node { mib: &[6, 11], name: "hw.machine_arch", value: Value::Str(|b| b.buf_str(framework::MACHINE_ARCH)) },
    Node { mib: &[6, 13], name: "hw.pagesizes", value: Value::Ulong(|| PAGESIZES[0]) },
];

/// Helpers for `Value::Str`.
trait BufStr {
    fn buf_str(&mut self, s: &str);
}

impl BufStr for FixedBuf<256> {
    fn buf_str(&mut self, s: &str) {
        let _ = core::fmt::Write::write_str(self, s);
    }
}

fn find_mib(mib: &[i32]) -> Option<&'static Node> {
    NODES.iter().find(|n| n.mib == mib)
}

fn find_name(name: &[u8]) -> Option<&'static Node> {
    NODES.iter().find(|n| n.name.as_bytes() == name)
}

/// Reads the old value into the user's buffer and applies the new one
/// (`sysctl_root`): with no buffer, the size is reported; a short buffer
/// gets what fits and `ENOMEM`.
fn access(p: &Proc, node: &Node, old: u64, oldlenp: u64, new: u64, newlen: u64) -> KResult<[u64; 2]> {
    // The value, encoded as FreeBSD's C type.
    let mut val = [0u8; 256];
    let mut s = FixedBuf::<256>::new();
    let len = match node.value {
        Value::Int(f) => {
            val[..4].copy_from_slice(&f().to_le_bytes());
            4
        }
        Value::Ulong(f) => {
            val[..8].copy_from_slice(&f().to_le_bytes());
            8
        }
        Value::Str(f) => {
            f(&mut s);
            let n = s.len.min(255);
            val[..n].copy_from_slice(&s.as_bytes()[..n]);
            n + 1 // with the NUL
        }
        Value::Hostname => {
            let mut h = [0u8; MAXHOSTNAMELEN];
            let n = hostname(&mut h);
            val[..n].copy_from_slice(&h[..n]);
            n + 1
        }
        // Filled below, as much as the caller asks for.
        Value::Arandom => 0,
        Value::Struct(f, _) => {
            let mut b = [0u8; 64];
            let n = f(&mut b).min(64);
            val[..n].copy_from_slice(&b[..n]);
            n
        }
    };
    if new != 0 {
        match node.value {
            Value::Hostname => {
                kern::vfs_subr::suser(&p.cred())?;
                let n = usize_from_u64(newlen).min(MAXHOSTNAMELEN + 1);
                let mut b = [0u8; MAXHOSTNAMELEN + 1];
                p.vm().copyin(new, &mut b[..n]).map_err(vm_errno)?;
                let len = b[..n].iter().position(|&c| c == 0).unwrap_or(n);
                set_hostname(&b[..len])?;
            }
            _ => return Err(Errno::EPERM),
        }
    }
    if oldlenp == 0 {
        return Ok([0, 0]);
    }
    let mut lenb = [0u8; 8];
    p.vm().copyin(oldlenp, &mut lenb).map_err(vm_errno)?;
    let room = usize_from_u64(u64::from_le_bytes(lenb));
    let want = if let Value::Arandom = node.value {
        // As much randomness as asked, up to the buffer FreeBSD uses.
        let n = room.min(val.len());
        kern_random::arc4random_buf(&mut val[..n]);
        n
    } else {
        len
    };
    let result = if old == 0 {
        Ok([0, 0])
    } else {
        let n = want.min(room);
        p.vm().copyout(old, &val[..n]).map_err(vm_errno)?;
        if n < want { Err(Errno::ENOMEM) } else { Ok([0, 0]) }
    };
    let reported = if old == 0 { want } else { want.min(room) };
    p.vm().copyout(oldlenp, &u64_from_usize(reported).to_le_bytes()).map_err(vm_errno)?;
    result
}

/// `{0, 3}`, `name2oid`: the numbers of a dotted name (what
/// `sysctlbyname` used before it was a system call).
fn name2oid(p: &Proc, old: u64, oldlenp: u64, new: u64, newlen: u64) -> KResult<[u64; 2]> {
    let mut name = [0u8; 256];
    let n = usize_from_u64(newlen).min(name.len());
    p.vm().copyin(new, &mut name[..n]).map_err(vm_errno)?;
    let node = find_name(&name[..n]).ok_or(Errno::ENOENT)?;
    let mut out = [0u8; 4 * CTL_MAXNAME];
    for (i, x) in node.mib.iter().enumerate() {
        out[4 * i..4 * i + 4].copy_from_slice(&x.to_le_bytes());
    }
    let len = 4 * node.mib.len();
    p.vm().copyout(old, &out[..len]).map_err(vm_errno)?;
    if oldlenp != 0 {
        p.vm().copyout(oldlenp, &u64_from_usize(len).to_le_bytes()).map_err(vm_errno)?;
    }
    Ok([0, 0])
}

fn unknown(p: &Proc, what: core::fmt::Arguments<'_>) -> Errno {
    if kern::init_main::bootverbose() {
        printf!("pid {} comm {}: sysctl {}: unknown\n", p.pid, p.comm(), what);
    }
    Errno::ENOENT
}

/// `__sysctl(name, namelen, old, oldlenp, new, newlen)`.
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_sysctl(p: &Proc, args: &[u64; 6]) -> KResult<[u64; 2]> {
    let (name, namelen) = (args[0], usize_from_u64(u64::from(trunc_u32(args[1]))));
    if !(2..=CTL_MAXNAME).contains(&namelen) {
        return Err(Errno::EINVAL);
    }
    let mut mib = [0i32; CTL_MAXNAME];
    let mut b = [0u8; 4 * CTL_MAXNAME];
    p.vm().copyin(name, &mut b[..4 * namelen]).map_err(vm_errno)?;
    for (i, m) in mib.iter_mut().enumerate().take(namelen) {
        *m = i32::from_le_bytes([b[4 * i], b[4 * i + 1], b[4 * i + 2], b[4 * i + 3]]);
    }
    let mib = &mib[..namelen];
    if mib == [0, 3] {
        return name2oid(p, args[2], args[3], args[4], args[5]);
    }
    // `kern.proc.pathname.<pid>` (`-1`: the caller).
    if let [1, 14, 12, pid] = *mib {
        return proc_pathname(p, pid, args[2], args[3]);
    }
    if let [1, 14, 7, pid] = *mib {
        return crate::freebsd_kinfo::sysctl_kern_proc_args(p, pid, args[2], args[3]);
    }
    if let [1, 14, op, ref rest @ ..] = *mib
        && op & !0x10 != 7
        && op & !0x10 <= 8
        && rest.len() <= 1
    {
        return crate::freebsd_kinfo::sysctl_kern_proc(p, op, rest.first().copied(), args[2], args[3]);
    }
    // The tree's own nodes (`sysctl(8)`): a name, the next name, a type.
    if let [0, op, ref oid @ ..] = *mib {
        return match op {
            1 => oid_name(p, oid, args[2], args[3]),
            2 | 7 => oid_next(p, oid, args[2], args[3]),
            4 => oid_fmt(p, oid, args[2], args[3]),
            _ => Err(unknown(p, format_args!("{mib:?}"))),
        };
    }
    let node = find_mib(mib).ok_or_else(|| unknown(p, format_args!("{mib:?}")))?;
    access(p, node, args[2], args[3], args[4], args[5])
}

/// `kern.proc.pathname`: the path a process executed, with its NUL.
fn proc_pathname(p: &Proc, pid: i32, old: u64, oldlenp: u64) -> KResult<[u64; 2]> {
    let target = if pid == -1 || u32::try_from(pid).is_ok_and(|n| n == p.pid) {
        None
    } else {
        let pid = u32::try_from(pid).map_err(|_| Errno::EINVAL)?;
        Some(kern::kern_proc::pfind(pid).ok_or(Errno::ESRCH)?)
    };
    let q: &Proc = target.as_deref().unwrap_or(p);
    let mut val = [0u8; 1024];
    let len = q.with_execpath(|path| {
        let n = path.len().min(val.len() - 1);
        val[..n].copy_from_slice(&path[..n]);
        n + 1
    });
    let vm = p.vm();
    let mut lenb = [0u8; 8];
    vm.copyin(oldlenp, &mut lenb).map_err(vm_errno)?;
    let room = usize_from_u64(u64::from_le_bytes(lenb));
    if old != 0 {
        if room < len {
            return Err(Errno::ENOMEM);
        }
        vm.copyout(old, &val[..len]).map_err(vm_errno)?;
    }
    vm.copyout(oldlenp, &u64_from_usize(len).to_le_bytes()).map_err(vm_errno)?;
    Ok([0, 0])
}

/// `__sysctlbyname(name, namelen, old, oldlenp, new, newlen)`.
#[inline(never)] // its buffers stay out of `syscall`'s frame
pub(crate) fn sys_sysctlbyname(p: &Proc, args: &[u64; 6]) -> KResult<[u64; 2]> {
    let mut name = [0u8; 256];
    let n = usize_from_u64(args[1]).min(name.len());
    p.vm().copyin(args[0], &mut name[..n]).map_err(vm_errno)?;
    let name = &name[..n];
    if name == b"kern.msgbuf" {
        return msgbuf(p, args[2], args[3]);
    }
    if name == b"kern.devname" {
        return devname(p, args[2], args[3], args[4], args[5]);
    }

    let node = find_name(name).ok_or_else(|| {
        unknown(p, format_args!("{}", core::str::from_utf8(name).unwrap_or("?")))
    })?;
    access(p, node, args[2], args[3], args[4], args[5])
}

/// `kern.msgbuf`: the kernel's message buffer, oldest first (`dmesg`).
fn msgbuf(p: &Proc, old: u64, oldlenp: u64) -> KResult<[u64; 2]> {
    let vm = p.vm();
    let len = kern::subr_prf::msgbuf_len();
    let mut lenb = [0u8; 8];
    vm.copyin(oldlenp, &mut lenb).map_err(vm_errno)?;
    let room = usize_from_u64(u64::from_le_bytes(lenb));
    let mut done = 0usize;
    if old != 0 {
        let mut chunk = [0u8; 512];
        while done < len.min(room) {
            let want = (len.min(room) - done).min(chunk.len());
            let n = kern::subr_prf::msgbuf_read(done, &mut chunk[..want]);
            if n == 0 {
                break;
            }
            vm.copyout(old + u64_from_usize(done), &chunk[..n]).map_err(vm_errno)?;
            done += n;
        }
    }
    let reported = if old == 0 { len } else { done };
    vm.copyout(oldlenp, &u64_from_usize(reported).to_le_bytes()).map_err(vm_errno)?;
    if old != 0 && done < len { Err(Errno::ENOMEM) } else { Ok([0, 0]) }
}

/// `kern.devname`: the name of the device number written in (`devname`,
/// `ttyname`).
fn devname(p: &Proc, old: u64, oldlenp: u64, new: u64, newlen: u64) -> KResult<[u64; 2]> {
    if new == 0 || newlen != 8 {
        return Err(Errno::EINVAL);
    }
    let vm = p.vm();
    let mut b = [0u8; 8];
    vm.copyin(new, &mut b).map_err(vm_errno)?;
    let name = kern::devfs::devname(u64::from_le_bytes(b)).ok_or(Errno::ENOENT)?;
    let mut lenb = [0u8; 8];
    vm.copyin(oldlenp, &mut lenb).map_err(vm_errno)?;
    let room = usize_from_u64(u64::from_le_bytes(lenb));
    let need = name.len() + 1;
    if old != 0 {
        if room < need {
            return Err(Errno::ENOMEM);
        }
        vm.copyout(old, name).map_err(vm_errno)?;
        vm.copyout(old + u64_from_usize(name.len()), &[0]).map_err(vm_errno)?;
    }
    vm.copyout(oldlenp, &u64_from_usize(need).to_le_bytes()).map_err(vm_errno)?;
    Ok([0, 0])
}

/// Answers a read of a sysctl whose value is `data`: its size with no
/// buffer, else as much as fits (`ENOMEM` if not all).
pub(crate) fn reply(p: &Proc, data: &[u8], old: u64, oldlenp: u64) -> KResult<[u64; 2]> {
    let vm = p.vm();
    if oldlenp == 0 {
        return Ok([0, 0]);
    }
    let mut lenb = [0u8; 8];
    vm.copyin(oldlenp, &mut lenb).map_err(vm_errno)?;
    let room = usize_from_u64(u64::from_le_bytes(lenb));
    let n = if old == 0 { data.len() } else { data.len().min(room) };
    if old != 0 {
        vm.copyout(old, data.get(..n).unwrap_or(&[])).map_err(vm_errno)?;
    }
    vm.copyout(oldlenp, &u64_from_usize(n).to_le_bytes()).map_err(vm_errno)?;
    if old != 0 && n < data.len() { Err(Errno::ENOMEM) } else { Ok([0, 0]) }
}

/// `kern.boottime`: a `struct timeval`.
fn boottime(b: &mut [u8; 64]) -> usize {
    b[..8].copy_from_slice(&kern::kern_clock::boottime().to_le_bytes());
    16
}

/// `vm.loadavg`: a `struct loadavg` (three fixed-point averages and
/// `FSCALE`). TODO(sched): compute them; they read 0.
fn loadavg(b: &mut [u8; 64]) -> usize {
    b[16..24].copy_from_slice(&2048u64.to_le_bytes());
    24
}

/// The node of `oid`, if it is one.
fn oid_node(oid: &[i32]) -> Option<&'static Node> {
    NODES.iter().find(|n| n.mib == oid)
}

/// `{0, 1, oid...}`: the node's dotted name.
fn oid_name(p: &Proc, oid: &[i32], old: u64, oldlenp: u64) -> KResult<[u64; 2]> {
    let n = oid_node(oid).ok_or(Errno::ENOENT)?;
    let mut b = [0u8; 128];
    let len = n.name.len().min(127);
    b[..len].copy_from_slice(&n.name.as_bytes()[..len]);
    reply(p, &b[..=len], old, oldlenp)
}

/// `{0, 4, oid...}`: the node's kind (type and flags) and format.
fn oid_fmt(p: &Proc, oid: &[i32], old: u64, oldlenp: u64) -> KResult<[u64; 2]> {
    let n = oid_node(oid).ok_or(Errno::ENOENT)?;
    let (kind, fmt) = n.value.kind_fmt();
    let mut b = [0u8; 64];
    b[..4].copy_from_slice(&kind.to_le_bytes());
    let len = fmt.len().min(59);
    b[4..4 + len].copy_from_slice(&fmt.as_bytes()[..len]);
    reply(p, &b[..4 + len + 1], old, oldlenp)
}

/// `{0, 2, oid...}`: the node after `oid` in the tree's order (numbers
/// compared one by one, a prefix first), for `sysctl -a`.
fn oid_next(p: &Proc, oid: &[i32], old: u64, oldlenp: u64) -> KResult<[u64; 2]> {
    let next = NODES.iter().filter(|n| n.mib > oid).min_by(|a, b| a.mib.cmp(b.mib)).ok_or(Errno::ENOENT)?;
    let mut b = [0u8; 4 * CTL_MAXNAME];
    for (i, x) in next.mib.iter().enumerate() {
        b[4 * i..4 * i + 4].copy_from_slice(&x.to_le_bytes());
    }
    reply(p, &b[..4 * next.mib.len()], old, oldlenp)
}
