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

//! The kernel's random number generator (FreeBSD's `random(4)` and
//! `arc4random(9)`): ChaCha20 with fast key erasure (`libkern::chacha20`),
//! seeded at boot from the machine's hardware source (`rdseed`/`rdrand`).
//!
//! TODO(random): an entropy pool fed by interrupt timing and the loader's
//! `/boot/entropy`, reseeding, and blocking until seeded on machines with
//! no hardware source.

use framework::sync::MtxSpin;
use libkern::chacha20::ChaChaRng;

use crate::printf;

static RNG: MtxSpin<Option<ChaChaRng>> = MtxSpin::new(None);

/// Seeds the generator (`random_harvestq_init` and the first reseed).
pub fn random_init(bootverbose: bool) {
    let mut seed = [0u8; 32];
    let hw = framework::cpu::cpu_random(&mut seed);
    if !hw {
        // Nothing to seed with: still usable, but predictable. Say so.
        printf!("random: no hardware entropy source; random numbers are predictable\n");
    } else if bootverbose {
        printf!("random: seeded from the CPU's hardware source\n");
    }
    *RNG.lock() = Some(ChaChaRng::new(seed));
}

/// Fills `out` with random bytes (`arc4random_buf`).
///
/// # Panics
///
/// If called before `random_init`.
pub fn arc4random_buf(out: &mut [u8]) {
    let mut rng = RNG.lock();
    let Some(r) = rng.as_mut() else {
        panic!("arc4random_buf: random_init has not run");
    };
    r.fill(out);
}
