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

//! Integer conversions, by name (Zig's `@intCast` and `@truncate`).
//!
//! Rust's `as` converts silently: it truncates, wraps and reinterprets
//! signs without saying so. The kernel does not use it (`clippy::
//! as_conversions` is denied everywhere else); a conversion is one of:
//!
//! * `From`/`TryFrom`, when Rust provides it (`u64::from(x_u32)`);
//! * a **widening** below that Rust's `From` does not provide only because
//!   of other platforms: Nova runs on 64-bit machines (checked at compile
//!   time here), where `usize` and `u64` are the same size;
//! * an explicit **truncation** (`trunc_u8` keeps the low 8 bits), where
//!   losing the high bits is the point.
//!
//! This module is the only place allowed to write `as`.
#![allow(clippy::as_conversions)]

const _: () = assert!(usize::BITS == 64, "Nova supports 64-bit machines only");

/// `usize` to `u64`: lossless on every machine Nova supports.
#[inline(always)]
pub const fn u64_from_usize(x: usize) -> u64 {
    x as u64
}

/// `u64` to `usize`: lossless on every machine Nova supports.
#[inline(always)]
pub const fn usize_from_u64(x: u64) -> usize {
    x as usize
}

/// `u32` to `usize`: lossless on every machine Nova supports.
#[inline(always)]
pub const fn usize_from_u32(x: u32) -> usize {
    x as usize
}

/// `u32` to `u64`, usable in `const fn` (`From` is not).
#[inline(always)]
pub const fn u64_from_u32(x: u32) -> u64 {
    x as u64
}

/// `u16` to `u64`, usable in `const fn` (`From` is not).
#[inline(always)]
pub const fn u64_from_u16(x: u16) -> u64 {
    x as u64
}

/// `u8` to `u64`, usable in `const fn` (`From` is not).
#[inline(always)]
pub const fn u64_from_u8(x: u8) -> u64 {
    x as u64
}

/// The low 32 bits of `x`, as a signed `int`: how a 64-bit register
/// holds a C `int` argument.
#[inline(always)]
pub const fn trunc_i32(x: u64) -> i32 {
    x as u32 as i32
}

/// The low 32 bits of `x`.
#[inline(always)]
pub const fn trunc_u32(x: u64) -> u32 {
    x as u32
}

/// The low 16 bits of `x`.
#[inline(always)]
pub const fn trunc_u16(x: u64) -> u16 {
    x as u16
}

/// The low 8 bits of `x`.
#[inline(always)]
pub const fn trunc_u8(x: u64) -> u8 {
    x as u8
}

/// The bits of `x` as an unsigned number (two's complement).
#[inline(always)]
pub const fn u64_from_bits_i64(x: i64) -> u64 {
    x as u64
}

/// The bits of `x` as a signed number (two's complement).
#[inline(always)]
pub const fn i64_from_bits_u64(x: u64) -> i64 {
    x as i64
}

/// The address of a function pointer, as a `usize`, for the MD layer (an
/// entry point to load into a register, descriptor or MSR). A macro
/// because Rust has no stable way to name "any function pointer" as a
/// type; write the pointer's type at the call site:
///
/// ```ignore
/// let entry: extern "C" fn(u64) -> ! = init_secondary;
/// let addr = libkern::fn_addr!(entry);
/// ```
#[macro_export]
macro_rules! fn_addr {
    ($f:expr) => {{
        #[allow(clippy::as_conversions)]
        let addr = $f as usize;
        addr
    }};
}

#[cfg(kani)]
mod verification {
    use super::*;

    #[kani::proof]
    fn widenings_are_lossless() {
        let a: usize = kani::any();
        assert_eq!(usize_from_u64(u64_from_usize(a)), a);
        let b: u64 = kani::any();
        assert_eq!(u64_from_usize(usize_from_u64(b)), b);
        let c: u32 = kani::any();
        assert_eq!(u64_from_usize(usize_from_u32(c)), u64::from(c));
    }

    #[kani::proof]
    fn truncations_keep_the_low_bits() {
        let x: u64 = kani::any();
        assert_eq!(u64::from(trunc_u32(x)), x & 0xffff_ffff);
        assert_eq!(u64::from(trunc_u16(x)), x & 0xffff);
        assert_eq!(u64::from(trunc_u8(x)), x & 0xff);
        assert_eq!(i64_from_bits_u64(u64_from_bits_i64(x.cast_signed())), x.cast_signed());
    }
}
