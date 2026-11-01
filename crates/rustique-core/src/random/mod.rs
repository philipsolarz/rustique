//! CPython's `random` generator, bit for bit: the MT19937 core of `Modules/_randommodule.c`
//! (`genrand_uint32`, `getrandbits`, `random`) and the pure-Python algorithms of
//! `Lib/random.py` built on it (`_randbelow_with_getrandbits`, the index logic of
//! `shuffle`/`sample`/`choices`), so that a kernel consumes the stream exactly as CPython
//! does and leaves the generator in the same state.
//!
//! The state is borrowed from the Python object (`RandomObject.index` and `.state`), so no
//! copy is made; callers hold whatever lock CPython holds around the same work (the
//! object's critical section on free-threaded builds).

/// Words of state.
pub const N: usize = 624;
const M: usize = 397;
const MATRIX_A: u32 = 0x9908_b0df;
const UPPER_MASK: u32 = 0x8000_0000;
const LOWER_MASK: u32 = 0x7fff_ffff;

/// A view of a `RandomObject`'s state: `int index; uint32_t state[N];`.
#[derive(Debug)]
pub struct Mt<'a> {
    pub index: &'a mut i32,
    pub mt: &'a mut [u32; N],
}

impl Mt<'_> {
    /// `genrand_uint32`.
    #[inline]
    pub fn next_u32(&mut self) -> u32 {
        if *self.index >= N as i32 {
            self.twist();
        }
        let mut y = self.mt[*self.index as usize];
        *self.index += 1;
        y ^= y >> 11;
        y ^= (y << 7) & 0x9d2c_5680;
        y ^= (y << 15) & 0xefc6_0000;
        y ^= y >> 18;
        y
    }

    #[cold]
    fn twist(&mut self) {
        let mt = &mut *self.mt;
        let mag = |y: u32| if y & 1 == 0 { 0 } else { MATRIX_A };
        for kk in 0..N - M {
            let y = (mt[kk] & UPPER_MASK) | (mt[kk + 1] & LOWER_MASK);
            mt[kk] = mt[kk + M] ^ (y >> 1) ^ mag(y);
        }
        for kk in N - M..N - 1 {
            let y = (mt[kk] & UPPER_MASK) | (mt[kk + 1] & LOWER_MASK);
            mt[kk] = mt[kk + M - N] ^ (y >> 1) ^ mag(y);
        }
        let y = (mt[N - 1] & UPPER_MASK) | (mt[0] & LOWER_MASK);
        mt[N - 1] = mt[M - 1] ^ (y >> 1) ^ mag(y);
        *self.index = 0;
    }

    /// `getrandbits(k)` for `1 <= k <= 64`: words from least to most significant, the last
    /// one shifted right to keep its top bits (`_random_Random_getrandbits_impl`).
    #[inline]
    pub fn getrandbits(&mut self, k: u32) -> u64 {
        debug_assert!((1..=64).contains(&k));
        if k <= 32 {
            return u64::from(self.next_u32() >> (32 - k));
        }
        let lo = u64::from(self.next_u32());
        let hi = u64::from(self.next_u32() >> (64 - k));
        lo | hi << 32
    }

    /// `getrandbits(k)` for `1 <= k <= 128` (same word order as [`Self::getrandbits`]).
    #[inline]
    pub fn getrandbits128(&mut self, k: u32) -> u128 {
        debug_assert!((1..=128).contains(&k));
        let mut out: u128 = 0;
        let mut rem = k;
        let mut shift = 0;
        while rem > 0 {
            let mut r = self.next_u32();
            if rem < 32 {
                r >>= 32 - rem;
            }
            out |= u128::from(r) << shift;
            shift += 32;
            rem = rem.saturating_sub(32);
        }
        out
    }

    /// `_randbelow_with_getrandbits(n)` for `1 <= n < 2**128`.
    #[inline]
    pub fn randbelow128(&mut self, n: u128) -> u128 {
        debug_assert!(n >= 1);
        if let Ok(n64) = u64::try_from(n) {
            return u128::from(self.randbelow(n64));
        }
        let k = 128 - n.leading_zeros();
        loop {
            let r = self.getrandbits128(k);
            if r < n {
                return r;
            }
        }
    }

    /// `random()` (genrand_res53).
    #[inline]
    pub fn random(&mut self) -> f64 {
        let a = self.next_u32() >> 5;
        let b = self.next_u32() >> 6;
        (f64::from(a) * 67_108_864.0 + f64::from(b)) * (1.0 / 9_007_199_254_740_992.0)
    }

    /// `Random._randbelow_with_getrandbits(n)` for `1 <= n < 2**64`:
    /// `k = n.bit_length(); r = getrandbits(k); while r >= n: r = getrandbits(k)`.
    #[inline]
    pub fn randbelow(&mut self, n: u64) -> u64 {
        debug_assert!(n >= 1);
        let k = 64 - n.leading_zeros();
        loop {
            let r = self.getrandbits(k);
            if r < n {
                return r;
            }
        }
    }

    /// The swaps of `Random.shuffle(x)` for `len(x) == n`, in order: `(i, j)` means
    /// `x[i], x[j] = x[j], x[i]`.
    pub fn shuffle_swaps(&mut self, n: usize, mut swap: impl FnMut(usize, usize)) {
        for i in (1..n).rev() {
            let j = self.randbelow(i as u64 + 1) as usize;
            swap(i, j);
        }
    }

    /// The population indices `Random.sample(population, k)` picks for `len(population)
    /// == n`, in selection order (`0 <= k <= n` is the caller's check). Follows CPython's
    /// choice between the pool and the set method exactly (see [`sample_uses_pool`]).
    pub fn sample_indices(&mut self, n: usize, k: usize) -> Vec<usize> {
        let mut out = Vec::with_capacity(k);
        if sample_uses_pool(n, k) {
            let mut pool: Vec<usize> = (0..n).collect();
            for i in 0..k {
                let j = self.randbelow((n - i) as u64) as usize;
                out.push(pool[j]);
                pool[j] = pool[n - i - 1];
            }
        } else {
            let mut selected = hashbrown::HashSet::with_capacity(k);
            for _ in 0..k {
                let mut j = self.randbelow(n as u64) as usize;
                while !selected.insert(j) {
                    j = self.randbelow(n as u64) as usize;
                }
                out.push(j);
            }
        }
        out
    }
}

/// `n <= setsize` in `Random.sample`, where `setsize = 21` plus, for `k > 5`,
/// `4 ** _ceil(_log(k * 3, 4))`.
///
/// CPython computes the exponent in floating point (`math.log(k*3)/math.log(4)`). `k*3` is
/// never a power of 4 (powers of 4 are 1 mod 3), so the exact value of the logarithm is
/// never an integer, and for `k < 2**40` its distance to the next integer (at least
/// `1/(4**m * ln 4)` with `4**m > 3k`) exceeds the error of the two correctly rounded `log`
/// calls and the division by more than 100x: the ceiling equals the smallest `m` with
/// `4**m >= k*3`, computed here in integers. Callers keep `k < 2**40` (property-tested
/// against the float formula below for every `k < 2**20` and sampled above).
pub fn sample_uses_pool(n: usize, k: usize) -> bool {
    debug_assert!((k as u64) < 1 << 40);
    let mut setsize: u128 = 21;
    if k > 5 {
        let target = k as u128 * 3;
        let mut p: u128 = 1;
        while p < target {
            p *= 4;
        }
        setsize += p;
    }
    (n as u128) <= setsize
}

/// The float formula of `Random.sample`, for tests.
pub fn sample_setsize_float(k: usize) -> u128 {
    let mut setsize: u128 = 21;
    if k > 5 {
        let e = ((k as f64 * 3.0).ln() / 4f64.ln()).ceil();
        setsize += 4u128.pow(e as u32);
    }
    setsize
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `init_genrand(5489)` (the reference seeding of MT19937).
    fn seeded(s: u32) -> ([u32; N], i32) {
        let mut mt = [0u32; N];
        mt[0] = s;
        for i in 1..N {
            mt[i] = 1_812_433_253u32.wrapping_mul(mt[i - 1] ^ (mt[i - 1] >> 30)).wrapping_add(i as u32);
        }
        (mt, N as i32)
    }

    #[test]
    fn reference_outputs() {
        let (mut mt, mut index) = seeded(5489);
        let mut g = Mt { index: &mut index, mt: &mut mt };
        // MT19937 reference values for seed 5489.
        assert_eq!(g.next_u32(), 3_499_211_612);
        assert_eq!(g.next_u32(), 581_869_302);
        assert_eq!(g.next_u32(), 3_890_346_734);
        for _ in 3..9999 {
            g.next_u32();
        }
        assert_eq!(g.next_u32(), 4_123_659_995);
    }

    #[test]
    fn getrandbits_word_order() {
        let (mut a, mut ia) = seeded(1);
        let (mut b, mut ib) = seeded(1);
        let mut ga = Mt { index: &mut ia, mt: &mut a };
        let mut gb = Mt { index: &mut ib, mt: &mut b };
        let lo = u64::from(gb.next_u32());
        let hi = u64::from(gb.next_u32() >> (64 - 40));
        assert_eq!(ga.getrandbits(40), lo | hi << 32);
        assert_eq!(ga.getrandbits(5), u64::from(gb.next_u32() >> 27));
        let lo = u64::from(gb.next_u32());
        let hi = u64::from(gb.next_u32());
        assert_eq!(ga.getrandbits(64), lo | hi << 32);
    }

    #[test]
    fn getrandbits128_matches_word_order() {
        let (mut a, mut ia) = seeded(3);
        let (mut b, mut ib) = seeded(3);
        let mut ga = Mt { index: &mut ia, mt: &mut a };
        let mut gb = Mt { index: &mut ib, mt: &mut b };
        for k in [1u32, 31, 32, 33, 63, 64, 65, 96, 100, 127, 128] {
            let mut want: u128 = 0;
            let words = k.div_ceil(32);
            let mut rem = k;
            for i in 0..words {
                let mut r = gb.next_u32();
                if rem < 32 {
                    r >>= 32 - rem;
                }
                want |= u128::from(r) << (32 * i);
                rem = rem.saturating_sub(32);
            }
            assert_eq!(ga.getrandbits128(k), want, "k={k}");
            if k <= 64 {
                // and the 64-bit variant consumes the same
                let (mut c, mut ic) = seeded(9);
                let (mut d, mut id) = seeded(9);
                let x = Mt { index: &mut ic, mt: &mut c }.getrandbits(k);
                let y = Mt { index: &mut id, mt: &mut d }.getrandbits128(k);
                assert_eq!(u128::from(x), y);
            }
        }
    }

    #[test]
    fn randbelow_rejects_like_cpython() {
        let (mut a, mut ia) = seeded(7);
        let (mut b, mut ib) = seeded(7);
        let mut ga = Mt { index: &mut ia, mt: &mut a };
        let mut gb = Mt { index: &mut ib, mt: &mut b };
        for n in [1u64, 2, 3, 5, 6, 7, 100, 1 << 31, (1 << 32) + 1, u64::MAX] {
            for _ in 0..50 {
                let k = 64 - n.leading_zeros();
                let mut r = gb.getrandbits(k);
                while r >= n {
                    r = gb.getrandbits(k);
                }
                assert_eq!(ga.randbelow(n), r);
            }
        }
        assert_eq!(ia, ib);
    }

    #[test]
    fn sample_setsize_matches_float_formula() {
        for k in 0..(1 << 20) {
            let pool = sample_uses_pool;
            let s = sample_setsize_float(k);
            // n just below, at and just above the threshold.
            for n in [s.saturating_sub(1), s, s + 1] {
                if (n as usize) >= k {
                    assert_eq!(pool(n as usize, k), n <= s, "k={k} n={n}");
                }
            }
        }
        let mut k: usize = 1 << 20;
        while (k as u64) < 1 << 40 {
            let s = sample_setsize_float(k);
            assert_eq!(pool_threshold(k), s, "k={k}");
            k = k * 5 / 4 + 1;
        }
    }

    fn pool_threshold(k: usize) -> u128 {
        // Largest n using the pool method.
        let mut lo = k as u128;
        let mut hi = 1u128 << 50;
        while lo < hi {
            let mid = (lo + hi).div_ceil(2);
            if sample_uses_pool(mid as usize, k) {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        lo
    }

    #[test]
    fn random_is_res53() {
        let (mut a, mut ia) = seeded(42);
        let mut g = Mt { index: &mut ia, mt: &mut a };
        for _ in 0..1000 {
            let x = g.random();
            assert!((0.0..1.0).contains(&x));
        }
    }
}
