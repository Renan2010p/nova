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

//! The early console (`mdi::EarlyCons`), available from the first
//! instruction. The MI console layer is `kern::kern_cons`.

use mdi::EarlyCons;

use crate::Md;

/// Writes one raw byte to the early console.
#[inline(always)]
pub fn early_putc(c: u8) {
    Md::putc(c)
}
