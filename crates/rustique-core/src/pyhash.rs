//! CPython-compatible numeric hashing (`Python/pyhash.c`, `Objects/longobject.c`).
//!
//! Python requires `hash(x) == hash(y)` whenever `x == y`, across `int`, `float` and
//! `bool`. CPython achieves this by hashing every rational number as its value modulo the
//! Mersenne prime `P = 2**61 - 1` (`2**31 - 1` on 32-bit platforms), with the sign applied
//! afterwards and `-1` (the C API error marker) replaced by `-2`. The functions here
//! reproduce `hash()` bit for bit, so natively stored numbers can live in hash tables
//! next to Python objects.

/// Number of bits of the hash modulus (`sys.hash_info.modulus == 2**HASH_BITS - 1`).
pub const HASH_BITS: u32 = if usize::BITS >= 64 { 61 } else { 31 };

/// `sys.hash_info.modulus`, the Mersenne prime all numeric hashes are reduced by.
pub const MODULUS: usize = (1 << HASH_BITS) - 1;

/// `hash(float('inf'))` (`sys.hash_info.inf`); `-inf` hashes to its negation.
pub const INF: isize = 314_159;

/// Apply the sign to a reduced magnitude and avoid the reserved value `-1`.
#[inline]
fn finish(magnitude: usize, negative: bool) -> isize {
    let x = if negative {
        magnitude.wrapping_neg()
    } else {
        magnitude
    };
    if x == usize::MAX { -2 } else { x as isize }
}

/// `n mod MODULUS`.
#[inline]
fn reduce_u64(n: u64) -> usize {
    if usize::BITS >= 64 {
        // 2**61 ≡ 1 (mod P): fold the top three bits onto the low 61.
        let p = MODULUS as u64;
        let r = (n & p) + (n >> HASH_BITS);
        (if r >= p { r - p } else { r }) as usize
    } else {
        (n % MODULUS as u64) as usize
    }
}

/// `hash(n)` for an `int` that fits in `i64`.
#[inline]
pub fn hash_i64(n: i64) -> isize {
    finish(reduce_u64(n.unsigned_abs()), n < 0)
}

/// `hash(n)` for an `int` that fits in `u64`.
#[inline]
pub fn hash_u64(n: u64) -> isize {
    finish(reduce_u64(n), false)
}

/// `hash(n)` for an `int` that fits in `i128`.
pub fn hash_i128(n: i128) -> isize {
    finish(reduce_u128(n.unsigned_abs()), n < 0)
}

/// `hash(n)` for an `int` that fits in `u128`.
pub fn hash_u128(n: u128) -> isize {
    finish(reduce_u128(n), false)
}

fn reduce_u128(n: u128) -> usize {
    (n % MODULUS as u128) as usize
}

/// `hash(b)` for a `bool`.
#[inline]
pub fn hash_bool(b: bool) -> isize {
    b as isize
}

/// `hash(x)` for a `float`, or `None` for NaN.
///
/// Since Python 3.10 a NaN hashes by identity (`object.__hash__`), so the caller must use
/// [`hash_pointer`] with the address of the Python object that holds the NaN.
pub fn hash_f64(v: f64) -> Option<isize> {
    if v.is_nan() {
        return None;
    }
    if v.is_infinite() {
        return Some(if v > 0.0 { INF } else { -INF });
    }
    // Port of `_Py_HashDouble`: hash the exact binary value `m * 2**e` modulo P.
    let (mut m, mut e) = frexp(v);
    let negative = m < 0.0;
    if negative {
        m = -m;
    }
    let bits = HASH_BITS as i32;
    let mut x: usize = 0;
    while m != 0.0 {
        // Process 28 bits at a time; multiplying by 2**28 is a rotation modulo P.
        x = ((x << 28) & MODULUS) | (x >> (bits - 28));
        m *= 268_435_456.0;
        e -= 28;
        let y = m as usize; // integer part, exact: 0 <= m < 2**28
        m -= y as f64;
        x += y;
        if x >= MODULUS {
            x -= MODULUS;
        }
    }
    // Multiply by 2**e: reduce e modulo HASH_BITS (2**HASH_BITS ≡ 1), then rotate.
    let e = if e >= 0 {
        e % bits
    } else {
        bits - 1 - ((-1 - e) % bits)
    } as u32;
    x = ((x << e) & MODULUS) | (x >> (HASH_BITS - e));
    Some(finish(x, negative))
}

/// `hash(float(x))` for an `f32` (the widening is exact), or `None` for NaN.
#[inline]
pub fn hash_f32(v: f32) -> Option<isize> {
    hash_f64(f64::from(v))
}

/// `object.__hash__` of the object at `addr` (`_Py_HashPointer`): the address rotated
/// right by four bits, since objects are at least 16-byte aligned.
#[inline]
pub fn hash_pointer(addr: usize) -> isize {
    let x = addr.rotate_right(4) as isize;
    if x == -1 { -2 } else { x }
}

/// Decompose a finite `v` into `m * 2**e` with `0.5 <= |m| < 1` (C `frexp`); zero maps to
/// `(v, 0)`.
fn frexp(v: f64) -> (f64, i32) {
    const EXP_MASK: u64 = 0x7ff << 52;
    if v == 0.0 || !v.is_finite() {
        return (v, 0);
    }
    let bits = v.to_bits();
    let biased = ((bits & EXP_MASK) >> 52) as i32;
    if biased == 0 {
        // Subnormal: scale into the normal range first (exact, a power of two).
        let (m, e) = frexp(v * f64::from_bits(0x43f0_0000_0000_0000)); // 2**64
        return (m, e - 64);
    }
    let m = f64::from_bits((bits & !EXP_MASK) | (1022 << 52));
    (m, biased - 1022)
}

#[cfg(all(test, target_pointer_width = "64"))]
mod tests {
    use super::*;
    use proptest::prelude::*;

    /// `(value, hash(value))` pairs printed by CPython 3.15.1.
    const INTS: &[(i128, isize)] = &[
        (0, 0),
        (1, 1),
        (-1, -2),
        (2, 2),
        (-2, -2),
        (3, 3),
        (-3, -3),
        (2305843009213693950, 2305843009213693950),
        (2305843009213693951, 0),
        (2305843009213693952, 1),
        (2305843009213693953, 2),
        (-2305843009213693951, 0),
        (-2305843009213693952, -2),
        (-2305843009213693953, -2),
        (4611686018427387904, 2),
        (4611686018427387903, 1),
        (4611686018427387902, 0),
        (-4611686018427387902, 0),
        (9223372036854775807, 3),
        (-9223372036854775808, -4),
        (9223372036854775808, 4),
        (18446744073709551615, 7),
        (18446744073709551616, 8),
        (-18446744073709551616, -8),
        (18446744073709551617, 9),
        (170141183460469231731687303715884105727, 31),
        (-170141183460469231731687303715884105728, -32),
        (15950735949418990474845684723364134912, 3),
        (1000000000000000000000000000000, 465258685558744706),
        (-1000000000000000000000000000000, -465258685558744706),
        (12345678901234567, 12345678901234567),
        (-98765432109876543, -98765432109876543),
    ];

    /// `(float bits, hash(float))` pairs printed by CPython 3.15.1.
    const FLOATS: &[(u64, isize)] = &[
        (0x0000000000000000, 0),                    // 0.0
        (0x8000000000000000, 0),                    // -0.0
        (0x3ff0000000000000, 1),                    // 1.0
        (0xbff0000000000000, -2),                   // -1.0
        (0x3fe0000000000000, 1152921504606846976),  // 0.5
        (0xbfe0000000000000, -1152921504606846976), // -0.5
        (0x3ff8000000000000, 1152921504606846977),  // 1.5
        (0x4000000000000000, 2),                    // 2.0
        (0x3fb999999999999a, 230584300921369408),   // 0.1
        (0xbfb999999999999a, -230584300921369408),  // -0.1
        (0x7e37e43c8800759c, 1224995262755759164),  // 1e300
        (0xfe37e43c8800759c, -1224995262755759164), // -1e300
        (0x01a56e1fc2f8f359, 482449582752280463),   // 1e-300
        (0x000730d67819e8d2, 1619072553786030470),  // 1e-308 (subnormal)
        (0x0000000000000001, 16777216),             // 5e-324
        (0x8000000000000001, -16777216),            // -5e-324
        (0x0010000000000000, 32768),                // smallest normal
        (0x000fffffffffffff, 2305843009196949503),  // largest subnormal
        (0x7fefffffffffffff, 2234066890152476671),  // max
        (0xffefffffffffffff, -2234066890152476671), // -max
        (0x400921fb54442d18, 326490430436040707),   // pi
        (0x43c0000000000000, 1),                    // 2**61
        (0x43d0000000000000, 2),                    // 2**62
        (0x43e0000000000000, 4),                    // 2**63
        (0x43f0000000000000, 8),                    // 2**64
        (0x3c20000000000000, 1),                    // 2**-61
        (0x3c10000000000000, 1152921504606846976),  // 2**-62
        (0x4341c37937e08000, 10000000000000000),    // 1e16
        (0x40fe240c9fbe76c9, 1819310134279660096),  // 123456.789
        (0xbe90c6f7a0b5ed8d, -976437270670017967),  // -2.5e-7
        (0x4340000000000000, 9007199254740992),     // 2**53
        (0x4330000000000002, 4503599627370498),     // 2**52 + 2
        (0x4480f0cf064dd592, 1864712049423028464),  // 1e22
        (0x44b52d02c7e14af6, 200376420512344424),   // 1e23
        (0x3fd5555555555555, 768614336404564608),   // 1/3
        (0xbff0000000000001, -513),                 // -1.0000000000000002
    ];

    #[test]
    fn ints_match_cpython() {
        for &(n, h) in INTS {
            assert_eq!(hash_i128(n), h, "hash({n})");
            if let Ok(n) = i64::try_from(n) {
                assert_eq!(hash_i64(n), h, "hash_i64({n})");
            }
            if let Ok(n) = u64::try_from(n) {
                assert_eq!(hash_u64(n), h, "hash_u64({n})");
            }
            if let Ok(n) = u128::try_from(n) {
                assert_eq!(hash_u128(n), h, "hash_u128({n})");
            }
        }
        assert_eq!(hash_u128(u128::MAX), 63);
        assert_eq!(hash_bool(true), 1);
        assert_eq!(hash_bool(false), 0);
    }

    #[test]
    fn floats_match_cpython() {
        for &(bits, h) in FLOATS {
            let v = f64::from_bits(bits);
            assert_eq!(hash_f64(v), Some(h), "hash({v:e})");
        }
        assert_eq!(hash_f64(f64::INFINITY), Some(314159));
        assert_eq!(hash_f64(f64::NEG_INFINITY), Some(-314159));
        assert_eq!(hash_f64(f64::NAN), None);
        assert_eq!(hash_f32(f32::NAN), None);
        assert_eq!(hash_f32(0.5), Some(1152921504606846976));
    }

    #[test]
    fn pointer_hash_rotates() {
        assert_eq!(hash_pointer(0x10), 1);
        assert_eq!(hash_pointer(0x7f00_0000_1230), 0x7f0_0000_0123);
        assert_eq!(hash_pointer(usize::MAX), -2);
    }

    #[test]
    fn frexp_matches_c() {
        assert_eq!(frexp(1.0), (0.5, 1));
        assert_eq!(frexp(-3.0), (-0.75, 2));
        assert_eq!(frexp(f64::from_bits(1)), (0.5, -1073));
        assert_eq!(frexp(0.0), (0.0, 0));
    }

    proptest! {
        #[test]
        fn integral_floats_hash_like_ints(n in any::<i64>()) {
            let f = n as f64;
            if f.abs() < 9.2e18 {
                prop_assert_eq!(hash_f64(f), Some(hash_i64(f as i64)));
            }
            prop_assert_eq!(hash_f64(f), Some(hash_i128(f as i128)));
        }

        #[test]
        fn ints_agree_across_widths(n in any::<i64>(), u in any::<u64>()) {
            prop_assert_eq!(hash_i64(n), hash_i128(n as i128));
            prop_assert_eq!(hash_u64(u), hash_u128(u as u128));
            prop_assert_eq!(hash_u64(u), hash_i128(u as i128));
            prop_assert_ne!(hash_i64(n), -1);
        }

        #[test]
        fn float_hash_is_odd_and_never_minus_one(bits in any::<u64>()) {
            let v = f64::from_bits(bits);
            match hash_f64(v) {
                None => prop_assert!(v.is_nan()),
                Some(h) => {
                    prop_assert_ne!(h, -1);
                    if h != -2 {
                        let neg = hash_f64(-v).unwrap();
                        prop_assert!(neg == -h || (h == 1 && neg == -2));
                    }
                }
            }
        }
    }
}
