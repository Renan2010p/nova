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

//! Kernel `printf` (FreeBSD's `kern/subr_prf.c`), on top of Rust's
//! `core::fmt` instead of a C format string.

use core::fmt::{self, Write};

use framework::sync::MtxSpin;

use crate::kern_cons::cnputs;

/// Serializes whole messages so lines from different CPUs don't mix.
static PRF_LOCK: MtxSpin<()> = MtxSpin::new(());

/// The console as a `fmt::Write`, for code that formats piecewise (each
/// piece is printed whole, but pieces from different CPUs may interleave).
pub struct Cons;

impl Write for Cons {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let _guard = PRF_LOCK.lock();
        cnputs(s);
        Ok(())
    }
}

/// The console without the lock. Only for `panic`, which may have
/// interrupted a CPU that holds it.
pub(crate) struct ConsUnlocked;

impl Write for ConsUnlocked {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        cnputs(s);
        Ok(())
    }
}

/// Back-end of [`printf!`](crate::printf). Use the macro.
#[doc(hidden)]
pub fn vprintf(args: fmt::Arguments<'_>) {
    let _guard = PRF_LOCK.lock();
    let _ = ConsUnlocked.write_fmt(args);
}

/// Prints without taking the console lock. Only for `panic`.
pub(crate) fn vprintf_unlocked(args: fmt::Arguments<'_>) {
    let _ = ConsUnlocked.write_fmt(args);
}

/// Prints to the kernel console, like `format!`.
#[macro_export]
macro_rules! printf {
    ($($arg:tt)*) => {
        $crate::subr_prf::vprintf(::core::format_args!($($arg)*))
    };
}
