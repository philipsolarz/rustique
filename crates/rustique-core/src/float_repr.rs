//! Exact and fast `repr(float)`: CPython's `float_repr` (`Objects/floatobject.c`), i.e.
//! `PyOS_double_to_string(x, 'r', 0, Py_DTSF_ADD_DOT_0, NULL)`.
//!
//! CPython prints the shortest decimal digit string that reads back as exactly `x` (David
//! Gay's `_Py_dg_dtoa` in mode 0: shortest round-trip digits, the candidate closest to `x`,
//! ties to even), laid out as
//! - fixed notation when the decimal exponent `e` of the leading digit is in `-4 ..= 15`
//!   (`0.0001`, `123.456`, `1000000000000000.0`), with `.0` appended to integral values;
//! - exponent notation otherwise (`1e+16`, `1.5e-05`, `5e-324`), the exponent signed and
//!   at least two digits wide;
//! - `-0.0` for negative zero; `inf`, `-inf` and `nan` (sign of a NaN ignored).
//!
//! The digits come from the Żmij algorithm (`zmij` crate: Schubfach-based shortest
//! round-trip with the same closest / ties-to-even choice as `dtoa` mode 0). Its own
//! layout differs from CPython's in two places only, fixed up here: it prints `e = -5` in
//! fixed notation (`0.00001234` vs `1.234e-05`) and does not pad the exponent (`1e+7` vs
//! `1e+07`). Identity with CPython is tested against CPython-recorded oracles
//! (`tests/float_repr_cpython.rs`) and, in-process, against `PyOS_double_to_string` itself
//! on hundreds of millions of values (`rustique._rustique._t_float._selfcheck`).

/// Longest `repr` of an `f64`: `-2.2250738585072014e-308` (24 bytes).
pub const MAX_LEN: usize = 24;

/// Write `repr(x)` for a **finite** `x` into `out` and return its length (ASCII bytes).
///
/// Non-finite input gives an unspecified (but in-bounds) result: callers check
/// `x.is_finite()` first (CPython's own path handles `inf`/`nan`).
#[inline]
pub fn write_finite(x: f64, out: &mut [u8; 32]) -> usize {
    let mut buf = zmij::Buffer::new();
    relayout(buf.format_finite(x).as_bytes(), out)
}

/// `repr(x)` for any `f64` (`inf`, `-inf`, `nan` included), as a `String`.
pub fn repr(x: f64) -> String {
    if x.is_nan() {
        return "nan".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "inf".into() } else { "-inf".into() };
    }
    let mut out = [0u8; 32];
    let n = write_finite(x, &mut out);
    // ASCII by construction.
    String::from_utf8_lossy(&out[..n]).into_owned()
}

/// Turn Żmij's layout into CPython's (see the module docs).
#[inline(always)]
fn relayout(s: &[u8], out: &mut [u8; 32]) -> usize {
    let n = s.len();
    debug_assert!(n <= MAX_LEN);
    // Exponent notation: "d[.ddd]e±X[X[X]]".
    if let Some(p) = s.iter().rposition(|&c| c == b'e') {
        // s[p + 1] is the sign; one exponent digit needs a leading zero.
        if n - p == 3 {
            out[..p + 2].copy_from_slice(&s[..p + 2]);
            out[p + 2] = b'0';
            out[p + 3] = s[p + 2];
            return n + 1;
        }
        out[..n].copy_from_slice(s);
        return n;
    }
    let neg = usize::from(s[0] == b'-');
    let body = &s[neg..];
    // Fixed notation with a leading-digit exponent of -5 ("0.0000d..." with d != 0; zero
    // prints as "0.0"): CPython uses exponent notation there.
    if body.len() > 6 && body.starts_with(b"0.0000") {
        let digits = &body[6..];
        let mut k = 0;
        if neg == 1 {
            out[0] = b'-';
            k = 1;
        }
        out[k] = digits[0];
        k += 1;
        if digits.len() > 1 {
            out[k] = b'.';
            k += 1;
            out[k..k + digits.len() - 1].copy_from_slice(&digits[1..]);
            k += digits.len() - 1;
        }
        out[k..k + 4].copy_from_slice(b"e-05");
        return k + 4;
    }
    out[..n].copy_from_slice(s);
    n
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pyfmt;
    use proptest::prelude::*;

    #[test]
    fn layout_rules() {
        let cases: &[(f64, &str)] = &[
            (0.0, "0.0"),
            (-0.0, "-0.0"),
            (1.0, "1.0"),
            (-1.5, "-1.5"),
            (0.1, "0.1"),
            (100.0, "100.0"),
            (1e15, "1000000000000000.0"),
            (1e16, "1e+16"),
            (1.5e16, "1.5e+16"),
            (9999999999999998.0, "9999999999999998.0"),
            (123456789012345678.0, "1.2345678901234568e+17"),
            (0.0001, "0.0001"),
            (0.00012345, "0.00012345"),
            (-0.00012345, "-0.00012345"),
            (0.00001, "1e-05"),
            (-0.00001, "-1e-05"),
            (1.5e-5, "1.5e-05"),
            (-1.2345e-5, "-1.2345e-05"),
            (9.999999999999999e-5, "9.999999999999999e-05"),
            (1e-6, "1e-06"),
            (1e-7, "1e-07"),
            (1e7, "10000000.0"),
            (1e22, "1e+22"),
            (1e-300, "1e-300"),
            (1e300, "1e+300"),
            (1e100, "1e+100"),
            (5e-324, "5e-324"),
            (-5e-324, "-5e-324"),
            (f64::MAX, "1.7976931348623157e+308"),
            (f64::MIN, "-1.7976931348623157e+308"),
            (f64::MIN_POSITIVE, "2.2250738585072014e-308"),
            (-f64::MIN_POSITIVE, "-2.2250738585072014e-308"),
            (0.30000000000000004, "0.30000000000000004"),
            (2.0f64.powi(53) + 2.0, "9007199254740994.0"),
            (2.0f64.powi(-25), "2.9802322387695312e-08"),
            (123.456, "123.456"),
            (f64::INFINITY, "inf"),
            (f64::NEG_INFINITY, "-inf"),
            (f64::NAN, "nan"),
            (-f64::NAN, "nan"),
        ];
        for &(x, want) in cases {
            assert_eq!(repr(x), want, "{x:e}");
        }
    }

    /// `(f64 bits, repr)` lines printed by CPython 3.15.1 (shared with `pyfmt`).
    #[test]
    fn matches_cpython_table() {
        let table = include_str!("pyfmt_cases.txt");
        let mut n = 0;
        for line in table.lines().filter(|l| !l.is_empty()) {
            let (bits, want) = line.split_once(' ').unwrap();
            let x = f64::from_bits(u64::from_str_radix(bits, 16).unwrap());
            assert_eq!(repr(x), want, "bits {bits}");
            n += 1;
        }
        assert!(n > 3000, "table has {n} cases");
    }

    #[test]
    fn longest_fits() {
        let mut out = [0u8; 32];
        for x in [-2.2250738585072014e-308, -1.2345678901234567e-300, -1.2345678901234567e-5] {
            assert!(write_finite(x, &mut out) <= MAX_LEN);
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(20_000))]

        /// Same text as the slow exact implementation (`pyfmt`, CPython-verified).
        #[test]
        fn agrees_with_pyfmt(bits in any::<u64>()) {
            let x = f64::from_bits(bits);
            prop_assert_eq!(repr(x), pyfmt::float_repr(x));
        }

        #[test]
        fn agrees_with_pyfmt_near_layout_cutoffs(m in 1u64..(1 << 53), e in -70i32..70) {
            let x = m as f64 * 2f64.powi(e);
            prop_assert_eq!(repr(x), pyfmt::float_repr(x));
            prop_assert_eq!(repr(-x), pyfmt::float_repr(-x));
        }

        #[test]
        fn round_trips(bits in any::<u64>()) {
            let x = f64::from_bits(bits);
            if x.is_finite() {
                prop_assert_eq!(repr(x).parse::<f64>().unwrap().to_bits(), x.to_bits());
            }
        }
    }
}
