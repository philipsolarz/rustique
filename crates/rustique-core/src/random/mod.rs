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

    /// `Random.normalvariate`'s rejection loop (Kinderman and Monahan), returning the
    /// accepted `z` (the method returns `mu + z * sigma`): `u1 = random()`,
    /// `u2 = 1.0 - random()`, `z = NV_MAGICCONST * (u1 - 0.5) / u2`, accepted when
    /// `z * z / 4.0 <= -_log(u2)`. Same operations in the same order (no fused
    /// multiply-add: Rust never contracts), `_log` = libm `log` ([`libm::log`]).
    /// `u2` lies in `[2**-53, 1]`, so an accepted `|z|` is at most `2 * sqrt(53 ln 2)`
    /// (< 12.2): see [`NORMAL_Z_BOUND`].
    #[inline]
    pub fn normal_z(&mut self, nv_magicconst: f64) -> f64 {
        loop {
            let u1 = self.random();
            let u2 = 1.0 - self.random();
            let z = nv_magicconst * (u1 - 0.5) / u2;
            let zz = z * z / 4.0;
            if zz <= -libm::log(u2) {
                return z;
            }
        }
    }

    /// `-_log(1.0 - self.random())`: `Random.expovariate` before the division by `lambd`.
    #[inline]
    pub fn expo(&mut self) -> f64 {
        -libm::log(1.0 - self.random())
    }

    /// `Random.paretovariate(alpha)` for `1/16 <= |alpha|` (finite), given
    /// `e = -1.0 / alpha`: `u = 1.0 - random(); u ** e` as `float_pow` computes it
    /// (`1.0 ** e` is 1.0, else libm `pow`). With `u` in `[2**-53, 1]` and
    /// `|e| <= 16` the result lies in `[2**-848, 2**848]`: never a range error.
    #[inline]
    pub fn pareto(&mut self, e: f64) -> f64 {
        let u = 1.0 - self.random();
        if u == 1.0 { 1.0 } else { libm::pow(u, e) }
    }

    /// `Random.gauss`'s new pair when `gauss_next` is `None`: `(z, next)` with
    /// `x2pi = random() * TWOPI`, `g2rad = _sqrt(-2.0 * _log(1.0 - random()))`,
    /// `z = _cos(x2pi) * g2rad`, `next = _sin(x2pi) * g2rad`.
    #[inline]
    pub fn gauss_pair(&mut self, twopi: f64) -> (f64, f64) {
        let x2pi = self.random() * twopi;
        let g2rad = libm::sqrt(-2.0 * libm::log(1.0 - self.random()));
        (libm::cos(x2pi) * g2rad, libm::sin(x2pi) * g2rad)
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
        let mut out = vec![0; k];
        self.sample_each(n, k, |i, j| out[i] = j);
        out
    }

    /// [`Self::sample_indices`] without building the result: `emit(i, j)` for the `i`-th
    /// pick `j`, in order. No heap allocation for the usual sizes: the pool method's
    /// index pool (`n <= 512`) and the set method's selected-index table (`k <= 256`)
    /// live on the stack.
    pub fn sample_each(&mut self, n: usize, k: usize, mut emit: impl FnMut(usize, usize)) {
        if sample_uses_pool(n, k) {
            // pool = list(population); result[i] = pool[j]; pool[j] = pool[n - i - 1]
            let mut stack = [std::mem::MaybeUninit::<u32>::uninit(); POOL_STACK];
            let mut heap: Vec<usize> = Vec::new();
            if n <= POOL_STACK {
                for (i, slot) in stack[..n].iter_mut().enumerate() {
                    slot.write(i as u32);
                }
                // SAFETY: the first `n` entries were just initialised.
                let pool = unsafe { &mut *(&raw mut stack[..n] as *mut [u32]) };
                for i in 0..k {
                    let j = self.randbelow((n - i) as u64) as usize;
                    emit(i, pool[j] as usize);
                    pool[j] = pool[n - i - 1];
                }
            } else {
                heap.extend(0..n);
                for i in 0..k {
                    let j = self.randbelow((n - i) as u64) as usize;
                    emit(i, heap[j]);
                    heap[j] = heap[n - i - 1];
                }
            }
        } else {
            // selected = set(); j = randbelow(n) until not in selected
            if k <= SET_STACK / 4 {
                let mut table = SmallSet::new(k);
                for i in 0..k {
                    let mut j = self.randbelow(n as u64) as usize;
                    while !table.insert(j) {
                        j = self.randbelow(n as u64) as usize;
                    }
                    emit(i, j);
                }
            } else {
                let mut selected = hashbrown::HashSet::with_capacity(k);
                for i in 0..k {
                    let mut j = self.randbelow(n as u64) as usize;
                    while !selected.insert(j) {
                        j = self.randbelow(n as u64) as usize;
                    }
                    emit(i, j);
                }
            }
        }
    }
}

/// Largest population whose index pool `sample` keeps on the stack (2 KiB).
const POOL_STACK: usize = 512;
/// Slots of the stack set of selected indices (8 KiB); used for `k <= SET_STACK / 4`.
const SET_STACK: usize = 1024;

/// An open-addressing set of indices on the stack, sized to a power of two at least `4k`
/// (load factor <= 1/4), Fibonacci hashing, linear probing; stores `j + 1` (0 = empty).
struct SmallSet {
    slots: [std::mem::MaybeUninit<u64>; SET_STACK],
    mask: usize,
    shift: u32,
}

impl SmallSet {
    fn new(k: usize) -> Self {
        let size = (4 * k).next_power_of_two().max(16);
        debug_assert!(size <= SET_STACK);
        let mut s = SmallSet { slots: [std::mem::MaybeUninit::uninit(); SET_STACK], mask: size - 1, shift: 64 - size.trailing_zeros() };
        for slot in &mut s.slots[..size] {
            slot.write(0);
        }
        s
    }

    /// Insert `j`; `false` if it was already there.
    #[inline]
    fn insert(&mut self, j: usize) -> bool {
        let key = j as u64 + 1;
        let mut h = (key.wrapping_mul(0x9e37_79b9_7f4a_7c15) >> self.shift) as usize;
        loop {
            // SAFETY: slots `0..=mask` were initialised in `new`.
            let v = unsafe { self.slots[h].assume_init() };
            if v == 0 {
                self.slots[h].write(key);
                return true;
            }
            if v == key {
                return false;
            }
            h = (h + 1) & self.mask;
        }
    }
}

/// Upper bound of `|z|` for an accepted [`Mt::normal_z`]: `zz <= -log(u2)` with
/// `u2 >= 2**-53` gives `|z| <= 2 * sqrt(53 * ln 2) = 12.12...`; rounded up.
pub const NORMAL_Z_BOUND: f64 = 12.5;

/// The C library functions `Lib/random.py` binds from `math` (`_log`, `_exp`, `_sqrt`,
/// `_cos`, `_sin`) and that `float_pow` calls, on the arguments the variates pass them
/// (finite, in the functions' domains, results in range: `math`'s error checks never
/// fire there, so the result is the C function's). Rust's `f64` methods lower to the same
/// C library calls (`log`, `exp`, `cos`, `sin`, `pow`; `sqrt` is the correctly rounded
/// IEEE operation everywhere); the Python extension checks them against `math` on probe
/// values before the first use.
pub mod libm {
    /// `log(x)` for `x > 0`.
    #[inline]
    pub fn log(x: f64) -> f64 {
        x.ln()
    }
    /// `exp(x)`.
    #[inline]
    pub fn exp(x: f64) -> f64 {
        x.exp()
    }
    /// `sqrt(x)` (IEEE, correctly rounded).
    #[inline]
    pub fn sqrt(x: f64) -> f64 {
        x.sqrt()
    }
    /// `cos(x)`.
    #[inline]
    pub fn cos(x: f64) -> f64 {
        x.cos()
    }
    /// `sin(x)`. The argument goes through `black_box` so that a `cos` and a `sin` of the
    /// same value stay two C calls, as in CPython (code generators may otherwise fuse
    /// them into one `sincos` call).
    #[inline]
    pub fn sin(x: f64) -> f64 {
        std::hint::black_box(x).sin()
    }
    /// `pow(x, y)`.
    #[inline]
    pub fn pow(x: f64, y: f64) -> f64 {
        x.powf(y)
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

    /// CPython 3.15.1 / 3.14.9 on the state `seeded(5489)` (`r.setstate((3, mt + (624,),
    /// None))`), `float` bit patterns.
    #[test]
    fn variates_match_cpython() {
        const NV: f64 = f64::from_bits(0x3ffb72cd3f331398);
        const TWOPI: f64 = f64::from_bits(0x401921fb54442d18);
        let fresh = || seeded(5489);
        let (mut mt, mut ix) = fresh();
        let mut g = Mt { index: &mut ix, mt: &mut mt };
        for want in [
            0x3fd01a580441a4ea_u64,
            0xbfead5f17279a132,
            0x3ff86243ce9ca735,
            0x3fe335ca32fd125e,
            0xbff97c509361d614,
            0x3fd1bb8d11ef70b5,
        ] {
            // normalvariate(0.0, 1.0) = 0.0 + z * 1.0
            assert_eq!((0.0 + g.normal_z(NV) * 1.0).to_bits(), want);
        }
        assert_eq!(ix, 44, "words consumed");
        let (mut mt, mut ix) = fresh();
        let mut g = Mt { index: &mut ix, mt: &mut mt };
        for want in [0x3ffaf9799940f04f_u64, 0x4002e5e311d9b563, 0x3fc1620bbc4e737d, 0x400391c5179acfcf] {
            assert_eq!((g.expo() / 1.0).to_bits(), want);
        }
        let (mut mt, mut ix) = fresh();
        let mut g = Mt { index: &mut ix, mt: &mut mt };
        let mut out = Vec::new();
        for _ in 0..2 {
            let (z, next) = g.gauss_pair(TWOPI);
            out.push(0.0 + z * 1.0);
            out.push(0.0 + next * 1.0);
        }
        let want = [0x3feb8341dd258761_u64, 0xbffff0ec537b9a11, 0x3ff8b5c7aad2ec64, 0x3ff955b898aae69e];
        assert_eq!(out.iter().map(|x| x.to_bits()).collect::<Vec<_>>(), want);
        let (mut mt, mut ix) = fresh();
        let mut g = Mt { index: &mut ix, mt: &mut mt };
        for want in [0x400295f38f0a799c_u64, 0x400a1075826e25bf, 0x3ff11fc9bc98092a, 0x400b2e6ad8f7cfec] {
            assert_eq!(g.pareto(-1.0 / 2.0).to_bits(), want);
        }
        let (mut mt, mut ix) = fresh();
        let mut g = Mt { index: &mut ix, mt: &mut mt };
        for want in [0x3ff2251be7253b3e_u64, 0x3fe50a40d56967ee, 0x400123ef961f4581, 0x3ff599e54e364750] {
            assert_eq!(libm::exp(0.0 + g.normal_z(NV) * 0.5).to_bits(), want);
        }
    }

    #[test]
    fn normal_z_bound_holds() {
        // The largest accepted |z| comes from the smallest u2 = 2**-53.
        let max = 2.0 * (-libm::log(2f64.powi(-53))).sqrt();
        assert!(max < NORMAL_Z_BOUND && max > 12.1, "{max}");
        let (mut mt, mut ix) = seeded(11);
        let mut g = Mt { index: &mut ix, mt: &mut mt };
        for _ in 0..200_000 {
            assert!(g.normal_z(f64::from_bits(0x3ffb72cd3f331398)).abs() <= max);
        }
    }

    /// The reference: CPython's `sample` with a Python `set` (here a `HashSet`) and a full
    /// pool, against [`Mt::sample_each`] (stack pool / stack set / heap variants).
    #[test]
    fn sample_each_matches_reference() {
        fn reference(g: &mut Mt<'_>, n: usize, k: usize) -> Vec<usize> {
            let mut out = Vec::new();
            if sample_uses_pool(n, k) {
                let mut pool: Vec<usize> = (0..n).collect();
                for i in 0..k {
                    let j = g.randbelow((n - i) as u64) as usize;
                    out.push(pool[j]);
                    pool[j] = pool[n - i - 1];
                }
            } else {
                let mut sel = std::collections::HashSet::new();
                for _ in 0..k {
                    let mut j = g.randbelow(n as u64) as usize;
                    while !sel.insert(j) {
                        j = g.randbelow(n as u64) as usize;
                    }
                    out.push(j);
                }
            }
            out
        }
        let cases = [(1, 0), (1, 1), (6, 2), (21, 5), (22, 5), (40, 5), (40, 40), (100, 6), (300, 20),
                     (512, 100), (513, 100), (600, 590), (10_000, 255), (10_000, 256), (10_000, 257),
                     (1_000_000, 20), (1_000_000, 300), (5_000, 1_000)];
        for (seed, &(n, k)) in cases.iter().enumerate() {
            let (mut a, mut ia) = seeded(seed as u32 + 100);
            let (mut b, mut ib) = seeded(seed as u32 + 100);
            let want = reference(&mut Mt { index: &mut ib, mt: &mut b }, n, k);
            let got = Mt { index: &mut ia, mt: &mut a }.sample_indices(n, k);
            assert_eq!(got, want, "n={n} k={k}");
            assert_eq!(ia, ib, "stream position n={n} k={k}");
            assert_eq!(a, b);
        }
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
