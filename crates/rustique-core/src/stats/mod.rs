//! Exact sums for `statistics`: CPython's `_sum`/`_ss` add every value as an exact
//! fraction; for `int` and finite `float` data every value is `±m·2^e` with `e >= -1074`,
//! so the sums are exact integers at a fixed binary scale. [`FixedAcc`] accumulates them
//! (separate positive and negative magnitudes, 64-bit limbs); the bindings turn the result
//! into Python ints and let CPython's correctly rounded integer division produce the float.

/// Scale of `Σx` for float data: every finite double is a multiple of `2**-1074`.
pub const SCALE_X: u32 = 1074;
/// Scale of `Σx²` for float data.
pub const SCALE_XX: u32 = 2 * SCALE_X;

/// Sum of `±m·2^shift` terms, exact.
#[derive(Debug, Clone)]
pub struct FixedAcc {
    pos: Vec<u64>,
    neg: Vec<u64>,
}

impl FixedAcc {
    /// Room for terms up to `2^max_bits` and `2^64` of them.
    pub fn new(max_bits: u32) -> Self {
        let limbs = (max_bits as usize + 64).div_ceil(64) + 2;
        FixedAcc { pos: vec![0; limbs], neg: vec![0; limbs] }
    }

    /// Add `(-1)^neg · m · 2^shift`.
    #[inline]
    pub fn add(&mut self, neg: bool, m: u128, shift: u32) {
        if m == 0 {
            return;
        }
        let acc = if neg { &mut self.neg } else { &mut self.pos };
        let (limb, off) = ((shift / 64) as usize, shift % 64);
        // m << off spans at most three limbs (m < 2^128).
        let lo = (m as u64) << off;
        let mid = if off == 0 { (m >> 64) as u64 } else { ((m >> (64 - off)) & u128::from(u64::MAX)) as u64 };
        let hi = if off == 0 { 0 } else { (m >> (128 - off)) as u64 };
        let mut carry = 0u64;
        for (k, v) in [lo, mid, hi].into_iter().enumerate() {
            let (s1, c1) = acc[limb + k].overflowing_add(v);
            let (s2, c2) = s1.overflowing_add(carry);
            acc[limb + k] = s2;
            carry = u64::from(c1) + u64::from(c2);
        }
        let mut k = limb + 3;
        while carry != 0 {
            let (s, c) = acc[k].overflowing_add(carry);
            acc[k] = s;
            carry = u64::from(c);
            k += 1;
        }
    }

    /// Little-endian bytes of the positive and negative magnitudes.
    pub fn to_le_bytes(&self) -> (Vec<u8>, Vec<u8>) {
        let f = |v: &[u64]| v.iter().flat_map(|l| l.to_le_bytes()).collect::<Vec<u8>>();
        (f(&self.pos), f(&self.neg))
    }
}

/// `x = (-1)^neg · m · 2^e` for a finite double (`m < 2^53`, `e >= -1074`).
#[inline]
pub fn decompose(x: f64) -> (bool, u64, i32) {
    let bits = x.to_bits();
    let neg = bits >> 63 != 0;
    let exp = ((bits >> 52) & 0x7ff) as i32;
    let frac = bits & ((1u64 << 52) - 1);
    if exp == 0 { (neg, frac, -1074) } else { (neg, frac | (1u64 << 52), exp - 1075) }
}

/// The terms of one value for `Σx` (at `scale_x`) and `Σx²` (at `2·scale_x`):
/// `(neg, m, shift)` each. Ints: `m = |i|`, exponent 0.
#[inline]
pub fn float_terms(x: f64) -> ((bool, u128, u32), (u128, u32)) {
    let (neg, m, e) = decompose(x);
    let sx = (neg, u128::from(m), (e + SCALE_X as i32) as u32);
    let sq = u128::from(m) * u128::from(m);
    ((sx.0, sx.1, sx.2), (sq, (2 * e + SCALE_XX as i32) as u32))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value(acc: &FixedAcc) -> f64 {
        // Only for small test values: limbs 0..3 as a float.
        let (p, n) = acc.to_le_bytes();
        let f = |b: &[u8]| {
            let mut v = 0f64;
            for (i, &x) in b.iter().enumerate().take(40) {
                v += f64::from(x) * 2f64.powi(8 * i as i32);
            }
            v
        };
        f(&p) - f(&n)
    }

    #[test]
    fn decompose_round_trips() {
        for x in [0.0, -0.0, 1.0, -2.5, 1e-310, 5e-324, f64::MAX, 0.1, -123456.789] {
            let (neg, m, e) = decompose(x);
            // Scale in two steps so subnormals do not underflow in 2^e.
            let v =
                if e < -500 { (m as f64) * 2f64.powi(e + 600) * 2f64.powi(-600) } else { (m as f64) * 2f64.powi(e) };
            assert_eq!(if neg { -v } else { v }, x);
        }
    }

    #[test]
    fn accumulates_exactly() {
        let mut a = FixedAcc::new(200);
        for (neg, m, s) in [(false, 3u128, 0u32), (false, 5, 64), (true, 1, 0), (false, u128::MAX, 10)] {
            a.add(neg, m, s);
        }
        let (p, n) = a.to_le_bytes();
        // pos = 3 + 5*2^64 + (2^128-1)*2^10, neg = 1
        let want_pos: u128 = 3 + (5u128 << 64);
        let low = u128::from_le_bytes(p[..16].try_into().unwrap());
        assert_eq!(low, want_pos.wrapping_add(u128::MAX << 10));
        assert_eq!(n[0], 1);
        let mut b = FixedAcc::new(64);
        b.add(false, 7, 1);
        b.add(true, 2, 0);
        assert_eq!(value(&b), 12.0);
    }
}
