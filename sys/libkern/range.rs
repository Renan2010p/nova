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

//! Address range checks: the arithmetic that stands between an address a
//! program gives and the kernel touching memory, kept here so it can be
//! proven once (`make kani`) and used everywhere.

/// The end of `[start, start + len)` if the range neither wraps around nor
/// goes past `limit` (FreeBSD's `vm_map_range_valid` and the
/// `addr + len > VM_MAXUSER_ADDRESS` tests).
pub const fn range_end(start: u64, len: u64, limit: u64) -> Option<u64> {
    match start.checked_add(len) {
        Some(end) if end <= limit => Some(end),
        _ => None,
    }
}

/// Like [`range_end`], and also requires `start` and `len` to be
/// multiples of `page`, a power of two.
pub const fn page_range_end(start: u64, len: u64, limit: u64, page: u64) -> Option<u64> {
    if start & (page - 1) != 0 || len & (page - 1) != 0 {
        return None;
    }
    range_end(start, len, limit)
}

/// Proofs: for every input, an accepted range lies entirely below the
/// limit, does not wrap, and nothing that does is accepted.
#[cfg(kani)]
mod verification {
    use super::*;

    #[kani::proof]
    fn range_end_is_exact() {
        let (start, len, limit): (u64, u64, u64) = kani::any();
        match range_end(start, len, limit) {
            Some(end) => {
                assert!(end >= start && end - start == len);
                assert!(end <= limit);
                // Every byte of the range is below the limit.
                assert!(len == 0 || start + (len - 1) < limit);
            }
            None => {
                assert!(start.checked_add(len).is_none_or(|e| e > limit));
            }
        }
    }

    #[kani::proof]
    fn page_range_end_is_aligned() {
        let (start, len, limit): (u64, u64, u64) = kani::any();
        let shift: u32 = kani::any_where(|&s| s < 64);
        let page = 1u64 << shift;
        if let Some(end) = page_range_end(start, len, limit, page) {
            assert!(start % page == 0 && len % page == 0 && end % page == 0);
            assert!(end <= limit && end - start == len);
        }
    }
}
