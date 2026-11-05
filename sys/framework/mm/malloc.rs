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

//! Explicit kernel memory allocation (FreeBSD's `malloc(9)`).
//!
//! The kernel has no global allocator and no crate links Rust's `alloc`:
//! nothing allocates behind the caller's back, and running out of memory
//! is an error the caller handles, never a panic. Every allocation says
//!
//! * **who** allocates: a [`MallocType`] (FreeBSD's `MALLOC_DEFINE`), which
//!   accounts the memory in use per subsystem (`vmstat -m`);
//! * **whether it may sleep**: exactly one of [`M_WAITOK`] (thread context,
//!   no spin lock or interrupt guard held; checked) or [`M_NOWAIT`] (any
//!   context), optionally with [`M_ZERO`];
//! * and it gets a `Result`: [`AllocError`] converts to `ENOMEM`.
//!
//! Owners: [`KBox`] (one value), [`KVec`] (a growable array whose growth
//! is itself an explicit, fallible allocation), [`KArc`] (shared, counted)
//! and [`KFnOnce`] (a closure to run once, e.g. a thread's body). Each
//! frees its memory, to the type it came from, when dropped.
//!
//! Until the page daemon exists, `M_WAITOK` cannot wait for memory to be
//! reclaimed, so it can fail too; callers handle that like any error.

use core::alloc::Layout;
use core::fmt;
use core::marker::PhantomData;
use core::mem::{ManuallyDrop, size_of};
use core::ops::{BitOr, Deref, DerefMut};
use core::ptr::{self, NonNull};
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering, fence};

use super::kmem;
use crate::cpu;

/// A kind of kernel memory, for accounting (`struct malloc_type`).
///
/// ```ignore
/// static M_PROC: MallocType = MallocType::new("proc");
/// ```
pub struct MallocType {
    name: &'static str,
    /// Bytes allocated and not yet freed.
    inuse: AtomicUsize,
    /// Successful allocations, ever.
    calls: AtomicU64,
}

impl MallocType {
    /// A malloc type named `name` (`MALLOC_DEFINE`).
    pub const fn new(name: &'static str) -> Self {
        Self {
            name,
            inuse: AtomicUsize::new(0),
            calls: AtomicU64::new(0),
        }
    }

    /// The type's name (`ks_shortdesc`).
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// Bytes in use (`MemUse` in `vmstat -m`).
    pub fn inuse(&self) -> usize {
        self.inuse.load(Ordering::Relaxed)
    }

    /// Allocations ever made (`Requests` in `vmstat -m`).
    pub fn calls(&self) -> u64 {
        self.calls.load(Ordering::Relaxed)
    }
}

impl fmt::Debug for MallocType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name)
    }
}

/// `malloc(9)` flags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MFlags(u32);

/// Never sleep; fail at once if memory is short.
pub const M_NOWAIT: MFlags = MFlags(0x0001);
/// The caller may sleep: thread context, no spin lock or guard held.
pub const M_WAITOK: MFlags = MFlags(0x0002);
/// Zero the memory.
pub const M_ZERO: MFlags = MFlags(0x0100);

impl MFlags {
    const fn has(self, f: MFlags) -> bool {
        self.0 & f.0 != 0
    }
}

impl BitOr for MFlags {
    type Output = MFlags;
    fn bitor(self, rhs: MFlags) -> MFlags {
        MFlags(self.0 | rhs.0)
    }
}

/// Out of memory (or a size no allocator can serve).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AllocError;

impl From<AllocError> for sys::errno::Errno {
    fn from(_: AllocError) -> Self {
        sys::errno::Errno::ENOMEM
    }
}

/// Checks the flags against the calling context.
fn check_flags(flags: MFlags) {
    assert!(
        flags.has(M_NOWAIT) != flags.has(M_WAITOK),
        "malloc: exactly one of M_NOWAIT and M_WAITOK is required"
    );
    if flags.has(M_WAITOK) {
        cpu::assert_no_guard("malloc(M_WAITOK)");
    }
}

/// Allocates `layout` for `mtype`. A zero-sized layout gets a dangling,
/// aligned pointer and no memory.
fn malloc(layout: Layout, mtype: &'static MallocType, flags: MFlags) -> Result<NonNull<u8>, AllocError> {
    check_flags(flags);
    if layout.size() == 0 {
        return NonNull::new(ptr::without_provenance_mut(layout.align())).ok_or(AllocError);
    }
    let p = kmem::kmem_alloc(layout).ok_or(AllocError)?;
    if flags.has(M_ZERO) {
        // SAFETY: `p` is a fresh block of at least `layout.size()` bytes.
        unsafe { ptr::write_bytes(p.as_ptr(), 0, layout.size()) };
    }
    mtype.inuse.fetch_add(layout.size(), Ordering::Relaxed);
    mtype.calls.fetch_add(1, Ordering::Relaxed);
    Ok(p)
}

/// Frees what [`malloc`] returned.
///
/// # Safety
///
/// `p` must come from `malloc(layout, mtype, _)` with this same `layout`
/// and `mtype`, be unused, and be freed only once.
unsafe fn free(p: NonNull<u8>, layout: Layout, mtype: &'static MallocType) {
    if layout.size() == 0 {
        return;
    }
    // SAFETY: forwarded to the caller.
    unsafe { kmem::kmem_free(p, layout) };
    mtype.inuse.fetch_sub(layout.size(), Ordering::Relaxed);
}

/// One owned value on the kernel heap (`Box`, made explicit).
pub struct KBox<T> {
    ptr: NonNull<T>,
    mtype: &'static MallocType,
    _owns: PhantomData<T>,
}

// SAFETY: a `KBox<T>` owns its `T` exclusively, like `T` itself.
unsafe impl<T: Send> Send for KBox<T> {}
// SAFETY: as above; `&KBox<T>` only gives `&T`.
unsafe impl<T: Sync> Sync for KBox<T> {}

impl<T> KBox<T> {
    /// Moves `value` to the heap.
    pub fn new(value: T, mtype: &'static MallocType, flags: MFlags) -> Result<Self, AllocError> {
        let p = malloc(Layout::new::<T>(), mtype, flags)?.cast::<T>();
        // SAFETY: `p` is fresh, aligned and large enough for a `T`.
        unsafe { p.as_ptr().write(value) };
        Ok(Self { ptr: p, mtype, _owns: PhantomData })
    }

    /// Moves the value back off the heap, freeing its memory.
    pub fn into_inner(self) -> T {
        let me = ManuallyDrop::new(self);
        // SAFETY: the value is initialized and owned; `me` is never dropped,
        // so it is read exactly once and the memory freed exactly once.
        unsafe {
            let value = me.ptr.as_ptr().read();
            free(me.ptr.cast(), Layout::new::<T>(), me.mtype);
            value
        }
    }
}

impl<T> Deref for KBox<T> {
    type Target = T;
    fn deref(&self) -> &T {
        // SAFETY: initialized and owned for the box's lifetime.
        unsafe { self.ptr.as_ref() }
    }
}

impl<T> DerefMut for KBox<T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: as in `deref`, and `&mut self` makes it exclusive.
        unsafe { self.ptr.as_mut() }
    }
}

impl<T> Drop for KBox<T> {
    fn drop(&mut self) {
        // SAFETY: the value is initialized and owned; after dropping it the
        // memory is freed once, as `new` allocated it.
        unsafe {
            ptr::drop_in_place(self.ptr.as_ptr());
            free(self.ptr.cast(), Layout::new::<T>(), self.mtype);
        }
    }
}

impl<T: fmt::Debug> fmt::Debug for KBox<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        (**self).fmt(f)
    }
}

/// A growable array on the kernel heap (`Vec`, made explicit): every
/// operation that may allocate takes flags and can fail.
pub struct KVec<T> {
    ptr: NonNull<T>,
    cap: usize,
    len: usize,
    mtype: &'static MallocType,
}

// SAFETY: a `KVec<T>` owns its elements exclusively.
unsafe impl<T: Send> Send for KVec<T> {}
// SAFETY: as above; `&KVec<T>` only gives `&[T]`.
unsafe impl<T: Sync> Sync for KVec<T> {}

impl<T> KVec<T> {
    const NOT_ZST: () = assert!(size_of::<T>() > 0, "KVec of a zero-sized type");

    /// An empty array; allocates nothing.
    pub const fn new(mtype: &'static MallocType) -> Self {
        let () = Self::NOT_ZST;
        Self { ptr: NonNull::dangling(), cap: 0, len: 0, mtype }
    }

    /// An empty array with room for `cap` elements.
    pub fn with_capacity(cap: usize, mtype: &'static MallocType, flags: MFlags) -> Result<Self, AllocError> {
        let mut v = Self::new(mtype);
        v.reserve(cap, flags)?;
        Ok(v)
    }

    /// Number of elements.
    pub fn len(&self) -> usize {
        self.len
    }

    /// True if there are no elements.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Elements that fit without allocating.
    pub fn capacity(&self) -> usize {
        self.cap
    }

    /// Makes room for `additional` more elements (at least doubling, so
    /// pushes are amortized constant time).
    pub fn reserve(&mut self, additional: usize, flags: MFlags) -> Result<(), AllocError> {
        let need = self.len.checked_add(additional).ok_or(AllocError)?;
        if need <= self.cap {
            return Ok(());
        }
        let cap = need.max(self.cap.saturating_mul(2)).max(4);
        let layout = Layout::array::<T>(cap).map_err(|_| AllocError)?;
        let new = malloc(layout, self.mtype, flags)?.cast::<T>();
        if self.cap > 0 {
            // SAFETY: the old buffer holds `len` initialized elements and the
            // new one has room for `cap >= len`; they do not overlap. The
            // elements are moved, so the old buffer is freed without dropping
            // them; it came from `malloc` with this layout and type.
            unsafe {
                ptr::copy_nonoverlapping(self.ptr.as_ptr(), new.as_ptr(), self.len);
                free(self.ptr.cast(), Layout::array::<T>(self.cap).map_err(|_| AllocError)?, self.mtype);
            }
        }
        self.ptr = new;
        self.cap = cap;
        Ok(())
    }

    /// Appends `value`, growing if needed. On failure `value` is dropped.
    pub fn push(&mut self, value: T, flags: MFlags) -> Result<(), AllocError> {
        self.reserve(1, flags)?;
        // SAFETY: `len < cap` after `reserve`; the slot is uninitialized.
        unsafe { self.ptr.as_ptr().add(self.len).write(value) };
        self.len += 1;
        Ok(())
    }

    /// Inserts `value` at `index`, shifting the rest up.
    ///
    /// # Panics
    ///
    /// If `index > len`.
    pub fn insert(&mut self, index: usize, value: T, flags: MFlags) -> Result<(), AllocError> {
        assert!(index <= self.len, "KVec::insert: index {index} > len {}", self.len);
        self.reserve(1, flags)?;
        // SAFETY: `len < cap`; the tail `[index, len)` moves up by one
        // inside the buffer, then the freed slot is written.
        unsafe {
            let at = self.ptr.as_ptr().add(index);
            ptr::copy(at, at.add(1), self.len - index);
            at.write(value);
        }
        self.len += 1;
        Ok(())
    }

    /// Removes and returns the element at `index`, shifting the rest down.
    ///
    /// # Panics
    ///
    /// If `index >= len`.
    pub fn remove(&mut self, index: usize) -> T {
        assert!(index < self.len, "KVec::remove: index {index} >= len {}", self.len);
        // SAFETY: `index < len`: the element is initialized; it is moved out
        // and the tail moves down over its slot.
        unsafe {
            let at = self.ptr.as_ptr().add(index);
            let value = at.read();
            ptr::copy(at.add(1), at, self.len - index - 1);
            self.len -= 1;
            value
        }
    }

    /// Removes and returns the last element.
    pub fn pop(&mut self) -> Option<T> {
        if self.len == 0 {
            return None;
        }
        self.len -= 1;
        // SAFETY: the element at the old `len - 1` is initialized and now
        // outside `len`, so it is moved out exactly once.
        Some(unsafe { self.ptr.as_ptr().add(self.len).read() })
    }

    /// Drops every element, keeping the memory.
    pub fn clear(&mut self) {
        let len = self.len;
        self.len = 0;
        // SAFETY: the first `len` elements were initialized and are no
        // longer reachable (`len` is 0 first, in case a drop panics).
        unsafe { ptr::drop_in_place(ptr::slice_from_raw_parts_mut(self.ptr.as_ptr(), len)) };
    }
}

impl<T: Copy> KVec<T> {
    /// Appends a copy of `items`.
    pub fn extend_from_slice(&mut self, items: &[T], flags: MFlags) -> Result<(), AllocError> {
        self.reserve(items.len(), flags)?;
        // SAFETY: room for `items.len()` more after `reserve`; `T: Copy`, and
        // `items` cannot overlap the uninitialized tail.
        unsafe {
            ptr::copy_nonoverlapping(items.as_ptr(), self.ptr.as_ptr().add(self.len), items.len());
        }
        self.len += items.len();
        Ok(())
    }
}

impl<T> Deref for KVec<T> {
    type Target = [T];
    fn deref(&self) -> &[T] {
        // SAFETY: the first `len` elements are initialized; `ptr` is aligned
        // and non-null (dangling only when `len == 0`).
        unsafe { core::slice::from_raw_parts(self.ptr.as_ptr(), self.len) }
    }
}

impl<T> DerefMut for KVec<T> {
    fn deref_mut(&mut self) -> &mut [T] {
        // SAFETY: as in `deref`, exclusive through `&mut self`.
        unsafe { core::slice::from_raw_parts_mut(self.ptr.as_ptr(), self.len) }
    }
}

impl<T> Drop for KVec<T> {
    fn drop(&mut self) {
        self.clear();
        if self.cap > 0
            && let Ok(layout) = Layout::array::<T>(self.cap)
        {
            // SAFETY: the buffer came from `malloc` with this layout (it was
            // valid then, so it is now) and type, and is freed once.
            unsafe { free(self.ptr.cast(), layout, self.mtype) };
        }
    }
}

impl<T: fmt::Debug> fmt::Debug for KVec<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        (**self).fmt(f)
    }
}

struct ArcInner<T> {
    count: AtomicUsize,
    mtype: &'static MallocType,
    value: T,
}

/// A shared value on the kernel heap, freed with its last reference
/// (`Arc`, made explicit). Cloning never allocates.
pub struct KArc<T> {
    ptr: NonNull<ArcInner<T>>,
    _owns: PhantomData<ArcInner<T>>,
}

// SAFETY: as `Arc`: the value is shared between threads (needs `Sync`)
// and may be dropped on any of them (needs `Send`).
unsafe impl<T: Send + Sync> Send for KArc<T> {}
// SAFETY: as above.
unsafe impl<T: Send + Sync> Sync for KArc<T> {}

impl<T> KArc<T> {
    /// Moves `value` to the heap with one reference.
    pub fn new(value: T, mtype: &'static MallocType, flags: MFlags) -> Result<Self, AllocError> {
        let p = malloc(Layout::new::<ArcInner<T>>(), mtype, flags)?.cast::<ArcInner<T>>();
        // SAFETY: fresh, aligned and large enough.
        unsafe { p.as_ptr().write(ArcInner { count: AtomicUsize::new(1), mtype, value }) };
        Ok(Self { ptr: p, _owns: PhantomData })
    }

    /// True if both point to the same value.
    pub fn ptr_eq(a: &Self, b: &Self) -> bool {
        a.ptr == b.ptr
    }

    /// The value, if `this` is its only reference (`Arc::try_unwrap`);
    /// `this` back otherwise. The heap memory is freed.
    pub fn try_unwrap(this: Self) -> Result<T, Self> {
        if this.inner().count.compare_exchange(1, 0, Ordering::Acquire, Ordering::Relaxed).is_err() {
            return Err(this);
        }
        let this = core::mem::ManuallyDrop::new(this);
        let mtype = this.inner().mtype;
        // SAFETY: the count went from 1 to 0, so no other reference exists
        // or can appear: the value is moved out once, and the memory freed
        // once without dropping it again (`this` is not dropped).
        unsafe {
            let value = ptr::read(&raw const (*this.ptr.as_ptr()).value);
            free(this.ptr.cast(), Layout::new::<ArcInner<T>>(), mtype);
            Ok(value)
        }
    }

    fn inner(&self) -> &ArcInner<T> {
        // SAFETY: alive while any reference, such as `self`, exists.
        unsafe { self.ptr.as_ref() }
    }
}

impl<T> Clone for KArc<T> {
    fn clone(&self) -> Self {
        let old = self.inner().count.fetch_add(1, Ordering::Relaxed);
        // As `Arc`: a count this large means references are being leaked.
        assert!(old <= isize::MAX.unsigned_abs(), "KArc: reference count overflow");
        Self { ptr: self.ptr, _owns: PhantomData }
    }
}

impl<T> Deref for KArc<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.inner().value
    }
}

impl<T> Drop for KArc<T> {
    fn drop(&mut self) {
        if self.inner().count.fetch_sub(1, Ordering::Release) != 1 {
            return;
        }
        // The last reference: see every other thread's last use first.
        fence(Ordering::Acquire);
        let mtype = self.inner().mtype;
        // SAFETY: no other reference exists, so the value is dropped and the
        // memory freed exactly once, as `new` allocated it.
        unsafe {
            ptr::drop_in_place(self.ptr.as_ptr());
            free(self.ptr.cast(), Layout::new::<ArcInner<T>>(), mtype);
        }
    }
}

impl<T: fmt::Debug> fmt::Debug for KArc<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        (**self).fmt(f)
    }
}

/// A closure on the kernel heap, to be run once (`Box<dyn FnOnce>`, made
/// explicit): a thread's body, for instance.
pub struct KFnOnce {
    data: NonNull<u8>,
    layout: Layout,
    mtype: &'static MallocType,
    /// Moves the closure out, frees its memory, and runs it.
    call: unsafe fn(NonNull<u8>, Layout, &'static MallocType),
    /// Drops the closure in place (it was never run).
    drop: unsafe fn(NonNull<u8>),
}

// SAFETY: built only from `Send` closures.
unsafe impl Send for KFnOnce {}

impl KFnOnce {
    /// Moves `f` to the heap.
    pub fn new<F: FnOnce() + Send + 'static>(
        f: F,
        mtype: &'static MallocType,
        flags: MFlags,
    ) -> Result<Self, AllocError> {
        let layout = Layout::new::<F>();
        let data = malloc(layout, mtype, flags)?;
        // SAFETY: fresh, aligned and large enough for an `F`.
        unsafe { data.cast::<F>().as_ptr().write(f) };
        Ok(Self { data, layout, mtype, call: call_impl::<F>, drop: drop_impl::<F> })
    }

    /// Runs the closure, consuming it.
    pub fn call(self) {
        let me = ManuallyDrop::new(self);
        // SAFETY: `data` holds an initialized `F` matching `call`; `me` is
        // never dropped, so the closure is moved out and run exactly once.
        unsafe { (me.call)(me.data, me.layout, me.mtype) }
    }
}

/// # Safety
///
/// `data` holds an initialized `F` allocated with `layout` for `mtype`,
/// used by nothing else.
unsafe fn call_impl<F: FnOnce()>(data: NonNull<u8>, layout: Layout, mtype: &'static MallocType) {
    // SAFETY: the caller's contract; the memory is freed before the closure
    // runs, so a closure that never returns (a thread body) leaks nothing.
    let f = unsafe {
        let f = data.cast::<F>().as_ptr().read();
        free(data, layout, mtype);
        f
    };
    f();
}

/// # Safety
///
/// `data` holds an initialized `F`, dropped here exactly once.
unsafe fn drop_impl<F>(data: NonNull<u8>) {
    // SAFETY: the caller's contract.
    unsafe { ptr::drop_in_place(data.cast::<F>().as_ptr()) };
}

impl Drop for KFnOnce {
    fn drop(&mut self) {
        // SAFETY: never run (`call` forgets `self`), so the closure is still
        // there; it is dropped and its memory freed once.
        unsafe {
            (self.drop)(self.data);
            free(self.data, self.layout, self.mtype);
        }
    }
}
