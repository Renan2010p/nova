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

//! The ChaCha20 stream cipher (RFC 8439), and a random number generator
//! built on it with fast key erasure, as FreeBSD's `arc4random(9)` and
//! Fortuna output stage do: after every request the key is replaced with
//! fresh keystream, so stealing the state never reveals earlier output.

/// "expand 32-byte k".
const SIGMA: [u32; 4] = [0x6170_7865, 0x3320_646e, 0x7962_2d32, 0x6b20_6574];

fn quarter(s: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize) {
    s[a] = s[a].wrapping_add(s[b]);
    s[d] = (s[d] ^ s[a]).rotate_left(16);
    s[c] = s[c].wrapping_add(s[d]);
    s[b] = (s[b] ^ s[c]).rotate_left(12);
    s[a] = s[a].wrapping_add(s[b]);
    s[d] = (s[d] ^ s[a]).rotate_left(8);
    s[c] = s[c].wrapping_add(s[d]);
    s[b] = (s[b] ^ s[c]).rotate_left(7);
}

/// One 64-byte block of keystream (RFC 8439, 2.3).
pub fn block(key: &[u8; 32], counter: u32, nonce: &[u8; 12]) -> [u8; 64] {
    let word = |b: &[u8], i: usize| {
        let mut w = [0u8; 4];
        w.copy_from_slice(&b[4 * i..4 * i + 4]);
        u32::from_le_bytes(w)
    };
    let mut init = [0u32; 16];
    init[..4].copy_from_slice(&SIGMA);
    for i in 0..8 {
        init[4 + i] = word(key, i);
    }
    init[12] = counter;
    for i in 0..3 {
        init[13 + i] = word(nonce, i);
    }
    let mut s = init;
    for _ in 0..10 {
        quarter(&mut s, 0, 4, 8, 12);
        quarter(&mut s, 1, 5, 9, 13);
        quarter(&mut s, 2, 6, 10, 14);
        quarter(&mut s, 3, 7, 11, 15);
        quarter(&mut s, 0, 5, 10, 15);
        quarter(&mut s, 1, 6, 11, 12);
        quarter(&mut s, 2, 7, 8, 13);
        quarter(&mut s, 3, 4, 9, 14);
    }
    let mut out = [0u8; 64];
    for (i, (w, i0)) in s.iter().zip(init.iter()).enumerate() {
        out[4 * i..4 * i + 4].copy_from_slice(&w.wrapping_add(*i0).to_le_bytes());
    }
    out
}

/// A cryptographically secure random number generator: ChaCha20 keystream
/// with fast key erasure.
pub struct ChaChaRng {
    key: [u8; 32],
    /// Distinguishes the requests made with one key (it changes after
    /// every request anyway).
    nonce: u64,
}

impl ChaChaRng {
    /// A generator keyed with `seed`, which must be secret and random.
    pub const fn new(seed: [u8; 32]) -> Self {
        Self { key: seed, nonce: 0 }
    }

    /// Mixes more entropy into the key (the new key is the keystream of
    /// the old key with `extra` as nonce, so neither alone predicts it).
    pub fn reseed(&mut self, extra: &[u8; 32]) {
        let mut n = [0u8; 12];
        n.copy_from_slice(&extra[..12]);
        let a = block(&self.key, 0, &n);
        for (k, (x, e)) in self.key.iter_mut().zip(a.iter().zip(extra.iter())) {
            *k = x ^ e;
        }
    }

    /// Fills `out` with random bytes, then replaces the key.
    pub fn fill(&mut self, out: &mut [u8]) {
        let mut nonce = [0u8; 12];
        nonce[..8].copy_from_slice(&self.nonce.to_le_bytes());
        self.nonce = self.nonce.wrapping_add(1);
        // Block 0 becomes the next key; the output starts at block 1.
        let first = block(&self.key, 0, &nonce);
        let mut counter = 1u32;
        for chunk in out.chunks_mut(64) {
            let b = block(&self.key, counter, &nonce);
            chunk.copy_from_slice(&b[..chunk.len()]);
            counter = counter.wrapping_add(1);
        }
        self.key.copy_from_slice(&first[..32]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 8439, 2.3.2.
    #[test]
    fn rfc8439_block() {
        let key: [u8; 32] = core::array::from_fn(|i| u8::try_from(i).unwrap());
        let nonce = [0, 0, 0, 0x09, 0, 0, 0, 0x4a, 0, 0, 0, 0];
        let b = block(&key, 1, &nonce);
        assert_eq!(&b[..16], &[0x10, 0xf1, 0xe7, 0xe4, 0xd1, 0x3b, 0x59, 0x15, 0x50, 0x0f, 0xdd, 0x1f, 0xa3, 0x20, 0x71, 0xc4]);
        assert_eq!(&b[48..], &[0xb5, 0x12, 0x9c, 0xd1, 0xde, 0x16, 0x4e, 0xb9, 0xcb, 0xd0, 0x83, 0xe8, 0xa2, 0x50, 0x3c, 0x4e]);
    }

    #[test]
    fn key_erasure() {
        let mut r = ChaChaRng::new([7; 32]);
        let (mut a, mut b) = ([0u8; 100], [0u8; 100]);
        r.fill(&mut a);
        r.fill(&mut b);
        assert_ne!(a, b, "the key changed between requests");
        assert_ne!(r.key, [7; 32]);
    }
}
