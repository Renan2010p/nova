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


//! Sockets (FreeBSD's `kern/uipc_socket.c`, `uipc_syscalls.c`) and the
//! local domain (`kern/uipc_usrreq.c`, `AF_UNIX`): communication between
//! processes on this machine, named in the file system or created as a
//! connected pair. The kernel names domains and types; the numbers are
//! the ABI's.
//!
//! Each socket has a receive queue of records: the bytes of one send, the
//! descriptors it passed (`SCM_RIGHTS`) and the sender's name. A stream
//! socket reads across records; datagram and sequenced-packet sockets
//! one record at a time. Sending to a connected socket appends to its
//! peer's queue. No code holds two sockets' locks at once (see
//! LOCKING.md): it takes what it needs from one, releases it, then goes
//! to the other.
//!
//! TODO(socket): other domains (`AF_INET`, `AF_INET6`), `SCM_CREDS`,
//! collecting cycles of descriptors in flight (`unp_gc`), `kqueue`.

use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use framework::mm::{KArc, KVec, M_WAITOK, MallocType};
use framework::sync::{Mutex, WaitQueue};
use sys::errno::{Errno, KResult};
use sys::signal::Signal;

use crate::kern_descrip::File;
use crate::kern_proc::{Cred, Proc};
use crate::kern_synch::{WChan, sleep_sig};
use crate::vnode::Vnode;

/// Sockets and their queues (`M_SOCKET`, `M_PCB`).
static M_SOCKET: MallocType = MallocType::new("socket");

/// Bytes a stream socket's receive queue holds before senders wait
/// (`net.local.stream.recvspace`... and `sendspace`).
pub const SOCKBUF: usize = 64 * 1024;
/// Datagrams a receive queue holds before senders get `ENOBUFS`.
pub const MAX_DGRAMS: usize = 128;
/// Largest datagram (`net.local.dgram.maxdgram`).
pub const MAX_DGRAM: usize = 16 * 1024;
/// Longest socket name (`sizeof(sun_path)`).
pub const SUN_PATH: usize = 104;

/// A communication domain (`PF_*`, by name).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Domain {
    /// This machine (`PF_LOCAL`, `PF_UNIX`).
    Local,
}

/// A socket type (`SOCK_*`, by name).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SockType {
    /// A connected byte stream (`SOCK_STREAM`).
    Stream,
    /// Separate messages, to any address (`SOCK_DGRAM`).
    Dgram,
    /// Separate messages over a connection (`SOCK_SEQPACKET`).
    SeqPacket,
}

/// One send, as queued at the receiver.
struct Record {
    data: KVec<u8>,
    /// Descriptors passed with it, delivered with its first byte.
    rights: KVec<KArc<File>>,
    /// The sender's name (datagrams), if it has one.
    from: KVec<u8>,
}

struct State {
    /// Its name, if bound: the file system node and the path given.
    bound: Option<(Vnode, KVec<u8>)>,
    /// Listening, with this many connections allowed to wait.
    backlog: Option<usize>,
    /// Connections waiting for `accept`.
    pending: KVec<KArc<Socket>>,
    /// The other end, if connected.
    peer: Option<KArc<Socket>>,
    /// Who the peer was when they connected (`LOCAL_PEERCRED`).
    peer_cred: Option<(Cred, u32)>,
    /// What was received and not yet read.
    rx: KVec<Record>,
    /// Bytes of the first record already read (stream sockets).
    rx_off: usize,
    /// No more sending (`shutdown(SHUT_WR)`).
    shut_wr: bool,
    /// The connection is over or no more receiving: reads see the end.
    eof: bool,
    /// Once connected.
    connected: bool,
}

/// A socket (`struct socket` with its `unpcb`).
pub struct Socket {
    /// Its domain.
    pub domain: Domain,
    /// Its type.
    pub kind: SockType,
    /// Who created it, and their pid (`so_cred`).
    pub cred: Cred,
    /// The creator's pid.
    pub pid: u32,
    st: Mutex<State>,
    /// Readers and acceptors sleep here, writers there.
    rq: KArc<WaitQueue>,
    wq: KArc<WaitQueue>,
    /// Bytes (or datagrams) queued, readable without the lock.
    rx_bytes: AtomicUsize,
    rx_recs: AtomicUsize,
    pending_n: AtomicUsize,
    eof: AtomicBool,
}

impl core::fmt::Debug for Socket {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Socket").field("kind", &self.kind).finish_non_exhaustive()
    }
}

/// Named sockets: the file system node a socket is bound to.
static BOUND: Mutex<KVec<(Vnode, KArc<Socket>)>> = Mutex::new(KVec::new(&M_SOCKET));

impl Socket {
    /// A new, unconnected socket (`socreate`).
    pub fn new(domain: Domain, kind: SockType, p: &Proc) -> KResult<KArc<Socket>> {
        Ok(KArc::new(
            Socket {
                domain,
                kind,
                cred: p.cred(),
                pid: p.pid,
                st: Mutex::new(State {
                    bound: None,
                    backlog: None,
                    pending: KVec::new(&M_SOCKET),
                    peer: None,
                    peer_cred: None,
                    rx: KVec::new(&M_SOCKET),
                    rx_off: 0,
                    shut_wr: false,
                    eof: false,
                    connected: false,
                }),
                rq: KArc::new(WaitQueue::new(), &M_SOCKET, M_WAITOK)?,
                wq: KArc::new(WaitQueue::new(), &M_SOCKET, M_WAITOK)?,
                rx_bytes: AtomicUsize::new(0),
                rx_recs: AtomicUsize::new(0),
                pending_n: AtomicUsize::new(0),
                eof: AtomicBool::new(false),
            },
            &M_SOCKET,
            M_WAITOK,
        )?)
    }

    fn connectionless(&self) -> bool {
        self.kind == SockType::Dgram
    }

    /// Whether a read would not sleep: data, the end, or a connection to
    /// accept.
    pub fn readable(&self) -> bool {
        self.rx_recs.load(Ordering::Acquire) > 0 || self.eof.load(Ordering::Acquire) || self.pending_n.load(Ordering::Acquire) > 0
    }

    /// Whether a write would not sleep (room at the peer, or an error
    /// waiting).
    pub fn writable(&self) -> bool {
        let peer = self.st.lock().peer.clone();
        match peer {
            Some(peer) => peer.rx_bytes.load(Ordering::Acquire) < SOCKBUF,
            None => self.connectionless() || self.eof.load(Ordering::Acquire),
        }
    }

    /// The other end is gone.
    pub fn hung_up(&self) -> bool {
        let st = self.st.lock();
        st.connected && st.peer.is_none()
    }

    /// Bytes ready to be read (`FIONREAD`).
    pub fn nread(&self) -> usize {
        self.rx_bytes.load(Ordering::Acquire)
    }

    /// Who the peer was when they connected (`LOCAL_PEERCRED`).
    pub fn peer_cred(&self) -> Option<(Cred, u32)> {
        self.st.lock().peer_cred
    }

    /// Whether it is listening.
    pub fn listening(&self) -> bool {
        self.st.lock().backlog.is_some()
    }

    /// Its own name, if bound.
    pub fn name(&self, out: &mut [u8; SUN_PATH]) -> usize {
        let st = self.st.lock();
        let Some((_, n)) = &st.bound else { return 0 };
        let k = n.len().min(SUN_PATH);
        out[..k].copy_from_slice(n.get(..k).unwrap_or(&[]));
        k
    }

    /// The peer's name, if connected to a bound socket.
    pub fn peer_name(&self, out: &mut [u8; SUN_PATH]) -> KResult<usize> {
        let peer = self.st.lock().peer.clone().ok_or(Errno::ENOTCONN)?;
        Ok(peer.name(out))
    }

    fn update(&self, st: &State) {
        let bytes: usize = st.rx.iter().map(|r| r.data.len()).sum::<usize>().saturating_sub(st.rx_off);
        self.rx_bytes.store(bytes, Ordering::Release);
        self.rx_recs.store(st.rx.len(), Ordering::Release);
        self.pending_n.store(st.pending.len(), Ordering::Release);
        self.eof.store(st.eof, Ordering::Release);
    }

    /// The last descriptor is gone (`soclose`): the peer sees the end,
    /// the name is freed, what was queued is dropped (descriptors in
    /// flight closed).
    pub(crate) fn close(this: &KArc<Self>) {
        let me = this;
        let (peer, pending, rx) = {
            let mut st = me.st.lock();
            st.eof = true;
            let peer = st.peer.take();
            let pending = core::mem::replace(&mut st.pending, KVec::new(&M_SOCKET));
            let rx = core::mem::replace(&mut st.rx, KVec::new(&M_SOCKET));
            st.bound = None;
            me.update(&st);
            (peer, pending, rx)
        };
        BOUND.lock().retain_ptr(me);
        if let Some(peer) = peer {
            {
                let mut ps = peer.st.lock();
                ps.peer = None;
                ps.eof = true;
                peer.update(&ps);
            }
            peer.rq.wake_all();
            peer.wq.wake_all();
        }
        // Connections never accepted end too.
        for s in pending.iter() {
            Socket::close(s);
        }
        drop(pending);
        drop(rx);
        me.rq.wake_all();
        me.wq.wake_all();
    }
}

trait RetainPtr {
    fn retain_ptr(&mut self, s: &KArc<Socket>);
}

impl RetainPtr for KVec<(Vnode, KArc<Socket>)> {
    /// Removes the entries of `s`.
    fn retain_ptr(&mut self, s: &KArc<Socket>) {
        let mut i = 0;
        while i < self.len() {
            if self.get(i).is_some_and(|e| KArc::ptr_eq(&e.1, s)) {
                drop(self.remove(i));
            } else {
                i += 1;
            }
        }
    }
}

/// Connects two new sockets to each other (`socketpair`).
pub fn kern_socketpair(a: &KArc<Socket>, b: &KArc<Socket>) {
    for (x, y) in [(a, b), (b, a)] {
        let mut st = x.st.lock();
        st.peer = Some(y.clone());
        st.peer_cred = Some((y.cred, y.pid));
        st.connected = true;
    }
}

/// Binds `s` to the file system node `vp` named `path` (`unp_bind`),
/// which the caller created; `EINVAL` if already bound.
pub fn kern_bind(s: &KArc<Socket>, vp: Vnode, path: &[u8]) -> KResult<()> {
    let mut name = KVec::with_capacity(path.len(), &M_SOCKET, M_WAITOK)?;
    name.extend_from_slice(path, M_WAITOK)?;
    {
        let mut st = s.st.lock();
        if st.bound.is_some() {
            return Err(Errno::EINVAL);
        }
        st.bound = Some((vp, name));
    }
    BOUND.lock().push((vp, s.clone()), M_WAITOK)?;
    Ok(())
}

/// The socket bound to `vp`, if any.
fn bound_at(vp: &Vnode) -> Option<KArc<Socket>> {
    BOUND.lock().iter().find(|(v, _)| v.same(vp)).map(|(_, s)| s.clone())
}

/// Starts listening (`solisten`): stream and sequenced-packet sockets.
pub fn kern_listen(s: &Socket, backlog: usize) -> KResult<()> {
    if s.connectionless() {
        return Err(Errno::EOPNOTSUPP);
    }
    let mut st = s.st.lock();
    if st.connected {
        return Err(Errno::EINVAL);
    }
    st.backlog = Some(backlog.clamp(1, 128));
    Ok(())
}

/// Connects `s` to the socket bound at `vp` (`unp_connect`): a stream
/// or sequenced-packet socket joins the listener's queue of connections;
/// a datagram socket only remembers where to send. `ECONNREFUSED` if no
/// one listens there.
pub fn kern_connect(p: &Proc, s: &KArc<Socket>, vp: &Vnode) -> KResult<()> {
    let target = bound_at(vp).ok_or(Errno::ECONNREFUSED)?;
    if target.kind != s.kind {
        return Err(Errno::EPROTOTYPE);
    }
    if s.connectionless() {
        let mut st = s.st.lock();
        st.peer = Some(target);
        st.connected = true;
        return Ok(());
    }
    if s.st.lock().connected {
        return Err(Errno::EISCONN);
    }
    // The listener's side of the connection: a new socket, as the
    // listener's (`sonewconn`).
    let server = KArc::new(
        Socket {
            domain: target.domain,
            kind: target.kind,
            cred: target.cred,
            pid: target.pid,
            st: Mutex::new(State {
                bound: None,
                backlog: None,
                pending: KVec::new(&M_SOCKET),
                peer: Some(s.clone()),
                peer_cred: Some((p.cred(), p.pid)),
                rx: KVec::new(&M_SOCKET),
                rx_off: 0,
                shut_wr: false,
                eof: false,
                connected: true,
            }),
            rq: KArc::new(WaitQueue::new(), &M_SOCKET, M_WAITOK)?,
            wq: KArc::new(WaitQueue::new(), &M_SOCKET, M_WAITOK)?,
            rx_bytes: AtomicUsize::new(0),
            rx_recs: AtomicUsize::new(0),
            pending_n: AtomicUsize::new(0),
            eof: AtomicBool::new(false),
        },
        &M_SOCKET,
        M_WAITOK,
    )?;
    {
        let mut ts = target.st.lock();
        let Some(backlog) = ts.backlog else { return Err(Errno::ECONNREFUSED) };
        if ts.pending.len() >= backlog {
            return Err(Errno::ECONNREFUSED);
        }
        ts.pending.push(server.clone(), M_WAITOK)?;
        target.update(&ts);
    }
    {
        let mut st = s.st.lock();
        st.peer = Some(server);
        st.peer_cred = Some((target.cred, target.pid));
        st.connected = true;
    }
    target.rq.wake_all();
    Ok(())
}

/// Takes the next connection (`kern_accept`): sleeps until one comes,
/// unless `nonblock` (`EAGAIN`).
pub fn kern_accept(p: &Proc, s: &KArc<Socket>, nonblock: bool) -> KResult<KArc<Socket>> {
    loop {
        {
            let mut st = s.st.lock();
            if st.backlog.is_none() {
                return Err(Errno::EINVAL);
            }
            if !st.pending.is_empty() {
                let c = st.pending.remove(0);
                s.update(&st);
                return Ok(c);
            }
        }
        if nonblock {
            return Err(Errno::EAGAIN);
        }
        sleep_sig(p, WChan::Shared(s.rq.clone()), || s.pending_n.load(Ordering::Acquire) > 0)?;
    }
}

/// Stops sending, receiving, or both (`soshutdown`).
pub fn kern_shutdown(s: &KArc<Socket>, rd: bool, wr: bool) -> KResult<()> {
    let peer = {
        let mut st = s.st.lock();
        if !st.connected {
            return Err(Errno::ENOTCONN);
        }
        if rd {
            st.eof = true;
            s.update(&st);
        }
        if wr {
            st.shut_wr = true;
        }
        st.peer.clone()
    };
    if let (true, Some(peer)) = (wr, peer) {
        let mut ps = peer.st.lock();
        ps.eof = true;
        peer.update(&ps);
        drop(ps);
        peer.rq.wake_all();
    }
    s.rq.wake_all();
    Ok(())
}

/// What a send carries besides bytes.
pub struct SendExtra<'a> {
    /// Descriptors to pass (`SCM_RIGHTS`).
    pub rights: KVec<KArc<File>>,
    /// Where to (datagrams to an unconnected socket's address).
    pub to: Option<&'a Vnode>,
    /// No `SIGPIPE` on a broken connection (`MSG_NOSIGNAL`).
    pub nosignal: bool,
    /// Do not sleep (`MSG_DONTWAIT`, `O_NONBLOCK`).
    pub nonblock: bool,
}

/// Sends `data` (`sosend`); returns the bytes taken. A stream send waits
/// for room, and may take part of `data`; a datagram goes whole or not
/// at all.
pub fn kern_send(p: &Proc, s: &KArc<Socket>, data: &[u8], mut extra: SendExtra<'_>) -> KResult<usize> {
    let (peer, shut, me_name) = {
        let st = s.st.lock();
        let mut name = KVec::new(&M_SOCKET);
        if let Some((_, n)) = &st.bound {
            name.extend_from_slice(n, M_WAITOK)?;
        }
        (st.peer.clone(), st.shut_wr, name)
    };
    let broken = |p: &Proc, nosignal: bool| {
        if !nosignal {
            crate::kern_sig::psignal(p, Signal::SIGPIPE);
        }
        Errno::EPIPE
    };
    if shut {
        return Err(broken(p, extra.nosignal));
    }
    let target = match (extra.to, peer) {
        (Some(vp), _) if s.connectionless() => bound_at(vp).ok_or(Errno::ECONNREFUSED)?,
        (Some(_), Some(_)) => return Err(Errno::EISCONN),
        (_, Some(peer)) => peer,
        (_, None) if s.connectionless() => return Err(Errno::EDESTADDRREQ),
        (_, None) => {
            return Err(if s.st.lock().connected { broken(p, extra.nosignal) } else { Errno::ENOTCONN });
        }
    };
    if s.kind != SockType::Stream && data.len() > MAX_DGRAM {
        return Err(Errno::EMSGSIZE);
    }
    let mut sent = 0;
    loop {
        {
            let mut ts = target.st.lock();
            if ts.eof && !s.connectionless() {
                drop(ts);
                return if sent > 0 { Ok(sent) } else { Err(broken(p, extra.nosignal)) };
            }
            let queued: usize = ts.rx.iter().map(|r| r.data.len()).sum();
            if s.kind == SockType::Stream {
                let room = SOCKBUF.saturating_sub(queued);
                if room > 0 {
                    let n = room.min(data.len() - sent);
                    let mut rec = Record { data: KVec::with_capacity(n, &M_SOCKET, M_WAITOK)?, rights: core::mem::replace(&mut extra.rights, KVec::new(&M_SOCKET)), from: KVec::new(&M_SOCKET) };
                    rec.data.extend_from_slice(data.get(sent..sent + n).unwrap_or(&[]), M_WAITOK)?;
                    ts.rx.push(rec, M_WAITOK)?;
                    target.update(&ts);
                    sent += n;
                }
            } else if ts.rx.len() < MAX_DGRAMS && queued + data.len() <= SOCKBUF {
                let mut rec = Record { data: KVec::with_capacity(data.len(), &M_SOCKET, M_WAITOK)?, rights: core::mem::replace(&mut extra.rights, KVec::new(&M_SOCKET)), from: KVec::new(&M_SOCKET) };
                rec.data.extend_from_slice(data, M_WAITOK)?;
                rec.from.extend_from_slice(&me_name, M_WAITOK)?;
                ts.rx.push(rec, M_WAITOK)?;
                target.update(&ts);
                sent = data.len();
            } else if s.connectionless() {
                return Err(Errno::ENOBUFS);
            }
        }
        target.rq.wake_all();
        if sent >= data.len() {
            return Ok(sent);
        }
        if extra.nonblock {
            return if sent > 0 { Ok(sent) } else { Err(Errno::EAGAIN) };
        }
        match sleep_sig(p, WChan::Shared(target.wq.clone()), || {
            target.rx_bytes.load(Ordering::Acquire) < SOCKBUF || target.eof.load(Ordering::Acquire)
        }) {
            Ok(()) => {}
            Err(e) => return if sent > 0 { Ok(sent) } else { Err(e) },
        }
    }
}

/// What a receive got besides bytes.
pub struct Received {
    /// Bytes copied.
    pub len: usize,
    /// Descriptors passed with them.
    pub rights: KVec<KArc<File>>,
    /// The sender's name (datagrams).
    pub from: KVec<u8>,
    /// The record was longer than the buffer (`MSG_TRUNC`).
    pub truncated: bool,
}

/// Receives into `buf` (`soreceive`): sleeps until something comes, the
/// end (0 bytes), or a signal. `peek` leaves it queued (`MSG_PEEK`).
pub fn kern_recv(p: &Proc, s: &KArc<Socket>, buf: &mut [u8], peek: bool, nonblock: bool) -> KResult<Received> {
    loop {
        {
            let mut st = s.st.lock();
            if !st.rx.is_empty() {
                let mut out = Received { len: 0, rights: KVec::new(&M_SOCKET), from: KVec::new(&M_SOCKET), truncated: false };
                if s.kind == SockType::Stream {
                    // Across records, from the first's unread part; the
                    // descriptors of each record come with its first byte.
                    while out.len < buf.len() && !st.rx.is_empty() {
                        let off = st.rx_off;
                        let Some(first) = st.rx.first_mut() else { break };
                        if off == 0 && !first.rights.is_empty() {
                            if out.len > 0 {
                                break; // one batch of descriptors per read
                            }
                            out.rights = core::mem::replace(&mut first.rights, KVec::new(&M_SOCKET));
                        }
                        let avail = first.data.get(off..).unwrap_or(&[]);
                        let n = avail.len().min(buf.len() - out.len);
                        buf.get_mut(out.len..out.len + n).ok_or(Errno::EFAULT)?.copy_from_slice(avail.get(..n).unwrap_or(&[]));
                        out.len += n;
                        if peek {
                            break;
                        }
                        if off + n >= first.data.len() {
                            drop(st.rx.remove(0));
                            st.rx_off = 0;
                        } else {
                            st.rx_off = off + n;
                        }
                    }
                } else {
                    let rec = if peek { None } else { Some(st.rx.remove(0)) };
                    let r = match &rec {
                        Some(r) => r,
                        None => st.rx.first().ok_or(Errno::EIO)?,
                    };
                    let n = r.data.len().min(buf.len());
                    buf.get_mut(..n).ok_or(Errno::EFAULT)?.copy_from_slice(r.data.get(..n).unwrap_or(&[]));
                    out.len = n;
                    out.truncated = r.data.len() > n;
                    out.from.extend_from_slice(&r.from, M_WAITOK)?;
                    if let Some(rec) = rec {
                        out.rights = rec.rights;
                    }
                }
                s.update(&st);
                drop(st);
                s.wq.wake_all();
                return Ok(out);
            }
            if st.eof || (st.connected && st.peer.is_none() && !s.connectionless()) {
                return Ok(Received { len: 0, rights: KVec::new(&M_SOCKET), from: KVec::new(&M_SOCKET), truncated: false });
            }
            if !st.connected && !s.connectionless() {
                return Err(Errno::ENOTCONN);
            }
        }
        if nonblock {
            return Err(Errno::EAGAIN);
        }
        sleep_sig(p, WChan::Shared(s.rq.clone()), || s.readable())?;
    }
}
