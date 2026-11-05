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


//! Pipes (FreeBSD's `kern/sys_pipe.c`): a buffer between two descriptors,
//! one end to read, one to write.
//!
//! A write of at most [`PIPE_BUF`] bytes is never interleaved with
//! another; a larger one may be. Reading with every writer gone gives end
//! of file; writing with every reader gone fails with `EPIPE` and sends
//! `SIGPIPE`. Both may sleep, and a signal ends the sleep.
//!
//! TODO(pipe): `select`/`poll`/`kqueue`, direct writes for large buffers.

use core::fmt;
use core::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};

use framework::mm::{KArc, KVec, M_WAITOK, MallocType};
use framework::sync::{Mutex, WaitQueue};
use sys::errno::{Errno, KResult};
use sys::signal::Signal;

use crate::kern_descrip::{File, FileFlags, FileKind};
use crate::kern_proc::Proc;
use crate::kern_synch::{WChan, sleep_sig};

/// Pipes (`M_PIPE`).
static M_PIPE: MallocType = MallocType::new("pipe");

/// Bytes a pipe holds (`PIPE_SIZE`).
pub const PIPE_SIZE: usize = 16384;
/// Writes at most this long are atomic (`PIPE_BUF`).
pub const PIPE_BUF: usize = 512;

/// File numbers for pipes, as `fstat` shows them.
static NEXT_INO: AtomicU64 = AtomicU64::new(1);

struct Ring {
    buf: KVec<u8>,
    head: usize,
    len: usize,
}

/// A pipe (`struct pipepair`).
pub struct Pipe {
    ring: Mutex<Ring>,
    /// Bytes buffered (as `ring.len`, readable without the lock).
    count: AtomicUsize,
    readers: AtomicU32,
    writers: AtomicU32,
    /// Readers sleep here, writers there.
    rq: KArc<WaitQueue>,
    wq: KArc<WaitQueue>,
    /// Its file number.
    pub ino: u64,
}

impl fmt::Debug for Pipe {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Pipe").field("ino", &self.ino).finish_non_exhaustive()
    }
}

/// Which end of a pipe a file is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PipeEnd {
    /// The end to read.
    Read,
    /// The end to write.
    Write,
}

impl Pipe {
    /// Bytes buffered.
    pub fn len(&self) -> usize {
        self.count.load(Ordering::Acquire)
    }

    /// Nothing buffered.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Every writer is gone: reads see the end of the file.
    pub fn writers_gone(&self) -> bool {
        self.writers.load(Ordering::Acquire) == 0
    }

    /// Every reader is gone: writes fail.
    pub fn readers_gone(&self) -> bool {
        self.readers.load(Ordering::Acquire) == 0
    }

    /// A write of `PIPE_BUF` bytes would not sleep.
    pub fn has_room(&self) -> bool {
        PIPE_SIZE - self.len() >= PIPE_BUF
    }

    /// One end closes (`pipeclose`): the other side is woken, to see the
    /// end of file or the broken pipe.
    pub(crate) fn close(&self, end: PipeEnd) {
        match end {
            PipeEnd::Read => self.readers.fetch_sub(1, Ordering::AcqRel),
            PipeEnd::Write => self.writers.fetch_sub(1, Ordering::AcqRel),
        };
        self.rq.wake_all();
        self.wq.wake_all();
    }
}

/// Creates a pipe (`kern_pipe`): returns its read and write descriptors.
pub fn kern_pipe(p: &Proc, cloexec: bool, nonblock: bool) -> KResult<(i32, i32)> {
    let mut buf = KVec::with_capacity(PIPE_SIZE, &M_PIPE, M_WAITOK)?;
    for _ in 0..PIPE_SIZE {
        buf.push(0, M_WAITOK)?;
    }
    let pipe = KArc::new(
        Pipe {
            ring: Mutex::new(Ring { buf, head: 0, len: 0 }),
            count: AtomicUsize::new(0),
            readers: AtomicU32::new(1),
            writers: AtomicU32::new(1),
            rq: KArc::new(WaitQueue::new(), &M_PIPE, M_WAITOK)?,
            wq: KArc::new(WaitQueue::new(), &M_PIPE, M_WAITOK)?,
            ino: NEXT_INO.fetch_add(1, Ordering::Relaxed),
        },
        &M_PIPE,
        M_WAITOK,
    )?;
    let flags = |read: bool| FileFlags { read, write: !read, append: false, nonblock };
    let rf = File::new(FileKind::Pipe(pipe.clone(), PipeEnd::Read), flags(true))?;
    let wf = File::new(FileKind::Pipe(pipe, PipeEnd::Write), flags(false))?;
    let r = p.fd.alloc(rf, 0, cloexec)?;
    match p.fd.alloc(wf, 0, cloexec) {
        Ok(w) => Ok((r, w)),
        Err(e) => {
            let _ = p.fd.close(r);
            Err(e)
        }
    }
}

/// Reads what is buffered, up to `out.len()` (`pipe_read`): sleeps while
/// the pipe is empty and has writers; 0 at end of file.
pub fn pipe_read(p: &Proc, pipe: &Pipe, out: &mut [u8], nonblock: bool) -> KResult<usize> {
    if out.is_empty() {
        return Ok(0);
    }
    loop {
        {
            let mut r = pipe.ring.lock();
            if r.len > 0 {
                let n = out.len().min(r.len);
                for (i, b) in out.iter_mut().take(n).enumerate() {
                    *b = r.buf.get((r.head + i) % PIPE_SIZE).copied().unwrap_or(0);
                }
                r.head = (r.head + n) % PIPE_SIZE;
                r.len -= n;
                pipe.count.store(r.len, Ordering::Release);
                drop(r);
                pipe.wq.wake_all();
                return Ok(n);
            }
        }
        if pipe.writers.load(Ordering::Acquire) == 0 {
            return Ok(0);
        }
        if nonblock {
            return Err(Errno::EAGAIN);
        }
        sleep_sig(p, WChan::Shared(pipe.rq.clone()), || {
            pipe.count.load(Ordering::Acquire) > 0 || pipe.writers.load(Ordering::Acquire) == 0
        })?;
    }
}

/// Writes `data` (at most [`PIPE_BUF`] bytes, so at once) (`pipe_write`):
/// sleeps until there is room for all of it. `EPIPE`, with `SIGPIPE`, if
/// no one reads.
pub fn pipe_write(p: &Proc, pipe: &Pipe, data: &[u8], nonblock: bool) -> KResult<usize> {
    let need = data.len().min(PIPE_BUF);
    loop {
        if pipe.readers.load(Ordering::Acquire) == 0 {
            crate::kern_sig::psignal(p, Signal::SIGPIPE);
            return Err(Errno::EPIPE);
        }
        {
            let mut r = pipe.ring.lock();
            if PIPE_SIZE - r.len >= need {
                let n = data.len().min(PIPE_SIZE - r.len);
                for (i, &b) in data.iter().take(n).enumerate() {
                    let at = (r.head + r.len + i) % PIPE_SIZE;
                    if let Some(slot) = r.buf.get_mut(at) {
                        *slot = b;
                    }
                }
                r.len += n;
                pipe.count.store(r.len, Ordering::Release);
                drop(r);
                pipe.rq.wake_all();
                return Ok(n);
            }
        }
        if nonblock {
            return Err(Errno::EAGAIN);
        }
        sleep_sig(p, WChan::Shared(pipe.wq.clone()), || {
            PIPE_SIZE - pipe.count.load(Ordering::Acquire) >= need || pipe.readers.load(Ordering::Acquire) == 0
        })?;
    }
}
