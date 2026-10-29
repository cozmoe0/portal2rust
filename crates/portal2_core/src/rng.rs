//! The engine's small random number generator (xorshift, per thread), for
//! gameplay randomness that needs no particular distribution.

use std::cell::Cell;

/// Uniform in `[0, 1)`.
pub fn rand_unit() -> f32 {
    thread_local!(static SEED: Cell<u64> = const { Cell::new(0x9E37_79B9_7F4A_7C15) });
    SEED.with(|s| {
        let mut x = s.get();
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        s.set(x);
        (x >> 40) as f32 / (1u64 << 24) as f32
    })
}

/// Uniform in `[lo, hi)`.
pub fn random_range(lo: f32, hi: f32) -> f32 {
    lo + rand_unit() * (hi - lo)
}
