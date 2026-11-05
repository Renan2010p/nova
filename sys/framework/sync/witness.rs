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


//! Lock order verification (FreeBSD's `WITNESS`, `kern/subr_witness.c`).
//!
//! Rust rules out data races; it does not rule out deadlocks. Two code
//! paths that take the same two locks in opposite orders can each hold one
//! and wait for the other. This module watches every [`MtxSpin`] and
//! [`Mutex`] acquisition and reports the first time an order contradicts
//! one seen before, directly or through other locks, as FreeBSD does:
//!
//! ```text
//! lock order reversal:
//!  1st kern/kern_proc.rs:310:22 (held)
//!  2nd kern/tty.rs:84:31 (taken)
//!  the order seen before: 2nd before 1st
//! ```
//!
//! A lock's *class* is where it was created ([`Location::caller`] in its
//! `new`): every process's signal lock is one class, the process table
//! another. Locks of the same class may nest (FreeBSD's `MTX_DUPOK`).
//! Spin locks held are tracked per CPU (interrupts are off while one is
//! held, so it cannot move); sleep mutexes per thread, as a thread may
//! sleep and move while holding one. Spin locks are checked against spin
//! locks and sleep mutexes against sleep mutexes: no order can lead from
//! a spin lock back to a sleep mutex (taking one with a spin lock held is
//! refused outright), so no cycle crosses the two kinds.
//!
//! Lock-free (atomics only), so locks taken while reporting are not
//! checked again, and it runs in any context. Each pair is reported once.
//!
//! [`MtxSpin`]: super::MtxSpin
//! [`Mutex`]: super::Mutex

use core::panic::Location;
use core::sync::atomic::{AtomicBool, AtomicPtr, AtomicU8, AtomicU64, Ordering};

use crate::pcpu::CpuLocal;
use crate::cpu::PinnedToCpu;

/// Most lock classes watched; more are not checked.
const MAX_CLASSES: usize = 256;
const WORDS: usize = MAX_CLASSES / 64;
/// Most locks one CPU, or one thread, holds at once that are tracked.
const MAX_HELD: usize = 16;

/// The classes, by number: where each was created.
static CLASSES: [AtomicPtr<Location<'static>>; MAX_CLASSES] =
    [const { AtomicPtr::new(core::ptr::null_mut()) }; MAX_CLASSES];
/// `ORDER[a]` bit `b`: a lock of class `a` was held when one of class `b`
/// was taken.
static ORDER: [[AtomicU64; WORDS]; MAX_CLASSES] = [const { [const { AtomicU64::new(0) }; WORDS] }; MAX_CLASSES];
/// `REPORTED[a]` bit `b`: the reversal of `a` and `b` was reported.
static REPORTED: [[AtomicU64; WORDS]; MAX_CLASSES] = [const { [const { AtomicU64::new(0) }; WORDS] }; MAX_CLASSES];
/// Set while a report is printed: locks taken then are not checked.
static REPORTING: AtomicBool = AtomicBool::new(false);
/// Reversals found so far.
static REVERSALS: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// How a reversal is reported: the class held, the class being taken.
pub type Reporter = fn(&'static Location<'static>, &'static Location<'static>);

/// Prints reversals; the kernel installs one that also prints a
/// backtrace ([`set_reporter`]). Set once, before other CPUs start.
static REPORTER: crate::sync::OnceLock<Reporter> = crate::sync::OnceLock::new();

/// Installs the function that prints reversals (the kernel's console).
pub fn set_reporter(f: Reporter) {
    let _ = REPORTER.set(f);
}

/// The locks held, by class number + 1 (0: empty slot).
pub(crate) struct Held {
    ids: [AtomicU8; MAX_HELD],
}

impl Held {
    pub(crate) const fn new() -> Self {
        Self { ids: [const { AtomicU8::new(0) }; MAX_HELD] }
    }

    fn push(&self, id: u8) {
        if let Some(slot) = self.ids.iter().find(|s| s.load(Ordering::Relaxed) == 0) {
            slot.store(id + 1, Ordering::Relaxed);
        }
    }

    fn remove(&self, id: u8) {
        if let Some(slot) = self.ids.iter().rev().find(|s| s.load(Ordering::Relaxed) == id + 1) {
            slot.store(0, Ordering::Relaxed);
        }
    }

    fn each(&self, mut f: impl FnMut(u8)) {
        for s in &self.ids {
            if let Some(id) = s.load(Ordering::Relaxed).checked_sub(1) {
                f(id);
            }
        }
    }
}

/// Spin locks held, per CPU.
static SPIN_HELD: CpuLocal<Held> = CpuLocal::new([const { Held::new() }; crate::MAXCPU]);

fn same(a: &Location<'_>, b: &Location<'_>) -> bool {
    a.line() == b.line() && a.column() == b.column() && a.file() == b.file()
}

/// The number of `loc`'s class, registering it the first time.
fn class(loc: &'static Location<'static>) -> Option<u8> {
    for (i, slot) in CLASSES.iter().enumerate() {
        let p = slot.load(Ordering::Acquire);
        // SAFETY: only `&'static Location` are ever stored.
        let known = unsafe { p.as_ref() };
        match known {
            Some(l) if same(l, loc) => return u8::try_from(i).ok(),
            Some(_) => continue,
            None => {
                let new = core::ptr::from_ref(loc).cast_mut();
                match slot.compare_exchange(core::ptr::null_mut(), new, Ordering::AcqRel, Ordering::Acquire) {
                    Ok(_) => return u8::try_from(i).ok(),
                    // Another CPU took the slot: it may be this class.
                    Err(other) => {
                        // SAFETY: as above.
                        if unsafe { other.as_ref() }.is_some_and(|l| same(l, loc)) {
                            return u8::try_from(i).ok();
                        }
                    }
                }
            }
        }
    }
    None
}

fn bit(m: &[[AtomicU64; WORDS]; MAX_CLASSES], a: u8, b: u8) -> bool {
    let (a, b) = (usize::from(a), usize::from(b));
    m.get(a).and_then(|row| row.get(b / 64)).is_some_and(|w| w.load(Ordering::Relaxed) & (1 << (b % 64)) != 0)
}

fn set_bit(m: &[[AtomicU64; WORDS]; MAX_CLASSES], a: u8, b: u8) {
    let (a, b) = (usize::from(a), usize::from(b));
    if let Some(w) = m.get(a).and_then(|row| row.get(b / 64)) {
        w.fetch_or(1 << (b % 64), Ordering::Relaxed);
    }
}

/// Whether the order graph leads from `from` to `to`.
fn reaches(from: u8, to: u8) -> bool {
    let mut seen = [0u64; WORDS];
    let mut stack = [0u8; MAX_CLASSES];
    let mut top = 1;
    stack[0] = from;
    while top > 0 {
        top -= 1;
        let a = stack[top];
        if a == to {
            return true;
        }
        let (w, m) = (usize::from(a) / 64, 1u64 << (a % 64));
        if seen[w] & m != 0 {
            continue;
        }
        seen[w] |= m;
        for (wi, word) in ORDER[usize::from(a)].iter().enumerate() {
            let mut bits = word.load(Ordering::Relaxed);
            while bits != 0 && top < MAX_CLASSES {
                let b = bits.trailing_zeros();
                bits &= bits - 1;
                if let Ok(next) = u8::try_from(wi * 64 + usize::try_from(b).unwrap_or(0)) {
                    stack[top] = next;
                    top += 1;
                }
            }
        }
    }
    false
}

fn report(held: u8, taking: u8) {
    if bit(&REPORTED, held, taking) || REPORTING.swap(true, Ordering::AcqRel) {
        return;
    }
    set_bit(&REPORTED, held, taking);
    REVERSALS.fetch_add(1, Ordering::Relaxed);
    let loc = |i: u8| {
        // SAFETY: only `&'static Location` are ever stored.
        CLASSES.get(usize::from(i)).and_then(|s| unsafe { s.load(Ordering::Acquire).as_ref() })
    };
    if let (Some(a), Some(b), Some(r)) = (loc(held), loc(taking), REPORTER.get()) {
        r(a, b);
    }
    REPORTING.store(false, Ordering::Release);
}

/// Checks taking a lock of class `taking` against every lock held.
fn check(taking: u8, held: &Held) {
    let mut look = |h: u8| {
        if h == taking || bit(&ORDER, h, taking) {
            return;
        }
        if reaches(taking, h) {
            report(h, taking);
        } else {
            set_bit(&ORDER, h, taking);
        }
    };
    held.each(&mut look);
}

/// A spin lock of class `loc` is about to be taken on this CPU (interrupts
/// are off: `pinned` proves it). Returns its class, to release with.
pub(crate) fn spin_acquire(loc: &'static Location<'static>, pinned: &impl PinnedToCpu) -> Option<u8> {
    if REPORTING.load(Ordering::Acquire) {
        return None;
    }
    let id = class(loc)?;
    let spins = SPIN_HELD.get(pinned);
    check(id, spins);
    spins.push(id);
    Some(id)
}

/// A spin lock of class `id` was released on this CPU.
pub(crate) fn spin_release(id: Option<u8>, pinned: &impl PinnedToCpu) {
    if let Some(id) = id {
        SPIN_HELD.get(pinned).remove(id);
    }
}

/// A sleep mutex of class `loc` is about to be taken by the current
/// thread (no spin lock is held: sleeping is allowed).
pub(crate) fn sleep_check(loc: &'static Location<'static>) -> Option<u8> {
    if REPORTING.load(Ordering::Acquire) {
        return None;
    }
    let id = class(loc)?;
    crate::task::with_current_witness(|t| check(id, t));
    Some(id)
}

/// The class of a sleep mutex created at `loc`, without checking (for a
/// try-lock).
pub(crate) fn sleep_class(loc: &'static Location<'static>) -> Option<u8> {
    if REPORTING.load(Ordering::Acquire) { None } else { class(loc) }
}

/// A sleep mutex of class `id` is now held by the current thread.
pub(crate) fn sleep_acquired(id: Option<u8>) {
    if let Some(id) = id {
        crate::task::with_current_witness(|t| t.push(id));
    }
}

/// A sleep mutex of class `id` was released by the current thread.
pub(crate) fn sleep_release(id: Option<u8>) {
    if let Some(id) = id {
        crate::task::with_current_witness(|t| t.remove(id));
    }
}

/// How many lock order reversals were found.
pub fn reversals() -> usize {
    REVERSALS.load(Ordering::Relaxed)
}

/// How many lock classes have been seen, and order relations recorded
/// (for `debug.witness` statistics).
pub fn stats() -> (usize, usize) {
    let classes = CLASSES.iter().filter(|s| !s.load(Ordering::Relaxed).is_null()).count();
    let edges = ORDER.iter().flatten().map(|w| usize::try_from(w.load(Ordering::Relaxed).count_ones()).unwrap_or(0)).sum();
    (classes, edges)
}
