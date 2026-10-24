//! CPython-compatible float formatting (`repr(float)`, `Python/pystrtod.c`).
//!
//! `repr(x)` prints the shortest decimal digit string that reads back as exactly `x`,
//! laid out in fixed notation when the decimal point position is in `-4 < decpt <= 16`
//! and in exponent notation (`1e+16`, `1e-05`) otherwise, with `.0` on integral values.
//!
//! Rust's `{:e}` produces shortest round-trip digits closest to the true value, like
//! David Gay's `dtoa` mode 0 used by CPython, except when the value lies exactly halfway
//! between two shortest candidates: Rust rounds the last digit up, CPython to even
//! (`repr(2**-25) == '2.9802322387695312e-08'`). [`shortest`] detects that case exactly
//! and applies CPython's choice.

use std::fmt::{self, Write};
use std::str::FromStr;

/// `repr(x)` for a Python `float`.
pub fn float_repr(x: f64) -> String {
    let mut out = String::with_capacity(24);
    push_float_repr(&mut out, x);
    out
}

/// `f32` counterpart of [`float_repr`]: the shortest digits that read back as the same
/// `f32`, with CPython's layout rules (`0.1`, `1e+16`, `3.4028235e+38`).
pub fn f32_repr(x: f32) -> String {
    let mut out = String::with_capacity(16);
    push_f32_repr(&mut out, x);
    out
}

/// Append `repr(x)` to `out`.
pub fn push_float_repr(out: &mut String, x: f64) {
    push_repr(out, x, x.is_nan(), x.is_sign_negative());
}

/// Append the `f32` repr of `x` to `out`.
pub fn push_f32_repr(out: &mut String, x: f32) {
    push_repr(out, x, x.is_nan(), x.is_sign_negative());
}

/// A binary floating-point type whose shortest decimal form we can compute.
trait Binary: Copy + PartialEq + fmt::LowerExp + FromStr {
    fn is_finite(self) -> bool;
    fn abs(self) -> Self;
    /// `(m, e)` with `self == m * 2**e`, for finite non-negative `self`.
    fn decompose(self) -> (u64, i32);
}

impl Binary for f64 {
    fn is_finite(self) -> bool {
        f64::is_finite(self)
    }
    fn abs(self) -> Self {
        f64::abs(self)
    }
    fn decompose(self) -> (u64, i32) {
        let bits = self.to_bits();
        let (biased, frac) = ((bits >> 52) as i32 & 0x7ff, bits & ((1 << 52) - 1));
        if biased == 0 {
            (frac, -1074)
        } else {
            (frac | 1 << 52, biased - 1075)
        }
    }
}

impl Binary for f32 {
    fn is_finite(self) -> bool {
        f32::is_finite(self)
    }
    fn abs(self) -> Self {
        f32::abs(self)
    }
    fn decompose(self) -> (u64, i32) {
        let bits = self.to_bits();
        let (biased, frac) = (
            (bits >> 23) as i32 & 0xff,
            u64::from(bits & ((1 << 23) - 1)),
        );
        if biased == 0 {
            (frac, -149)
        } else {
            (frac | 1 << 23, biased - 150)
        }
    }
}

/// `digits * 10**exp`, with `digits` free of trailing zeros (or zero).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Decimal {
    digits: u128,
    exp: i32,
}

impl Decimal {
    fn normalized(mut digits: u128, mut exp: i32) -> Self {
        while digits != 0 && digits.is_multiple_of(10) {
            digits /= 10;
            exp += 1;
        }
        Decimal { digits, exp }
    }
}

fn push_repr<F: Binary>(out: &mut String, x: F, nan: bool, negative: bool) {
    if !x.is_finite() {
        // CPython ignores the sign of a NaN.
        out.push_str(match (nan, negative) {
            (true, _) => "nan",
            (false, false) => "inf",
            (false, true) => "-inf",
        });
        return;
    }
    if negative {
        out.push('-');
    }
    layout(out, shortest(x.abs()));
}

/// The shortest decimal that reads back as `x` (finite, non-negative), ties to even.
fn shortest<F: Binary>(x: F) -> Decimal {
    let mut buf = StackBuf::new();
    let _ = write!(buf, "{x:e}");
    let (mantissa, exp) = buf.as_str().split_once('e').unwrap_or(("0", "0"));
    let (mut digits, mut n) = (0u128, 0i32);
    for b in mantissa.bytes().filter(u8::is_ascii_digit) {
        digits = digits * 10 + u128::from(b - b'0');
        n += 1;
    }
    let rust = Decimal::normalized(digits, exp.parse::<i32>().unwrap_or(0) - (n - 1));

    // Halfway case: x == (low + 1/2) * 10**e exactly, where `low` and `low + 1` both have
    // the shortest length. CPython keeps the even candidate if it also reads back as x.
    let Some(exact) = exact_decimal(x.decompose()) else {
        return rust;
    };
    if exact.digits % 10 != 5 {
        return rust;
    }
    let (low, e) = (exact.digits / 10, exact.exp + 1);
    let Some(scaled) = u32::try_from(rust.exp - e)
        .ok()
        .and_then(|k| 10u128.checked_pow(k))
    else {
        return rust;
    };
    let other = match rust.digits.checked_mul(scaled) {
        Some(r) if r == low + 1 && r % 2 == 1 => low,
        Some(r) if r == low && r % 2 == 1 => low + 1,
        _ => return rust,
    };
    let mut buf = StackBuf::new();
    let _ = write!(buf, "{other}e{e}");
    match buf.as_str().parse::<F>() {
        Ok(y) if y == x => Decimal::normalized(other, e),
        _ => rust,
    }
}

/// The exact decimal value of `m * 2**e` when it has at most 20 significant digits
/// (the only values that can be halfway between two candidates of <= 17 digits).
fn exact_decimal((m, e): (u64, i32)) -> Option<Decimal> {
    if m == 0 {
        return None;
    }
    let tz = m.trailing_zeros();
    let (m, e) = (u128::from(m >> tz), e + tz as i32);
    // m < 2**53, so neither product overflows. Outside these exponent ranges the exact
    // value needs more than 18 significant digits, too many for a halfway case.
    let d = match e {
        0..=74 => Decimal::normalized(m << e, 0),
        -27..=-1 => Decimal::normalized(m * 5u128.pow(e.unsigned_abs()), e),
        _ => return None,
    };
    (d.digits < 10u128.pow(20)).then_some(d)
}

/// Write `d` the way CPython's `format_float_short(x, 'r', 0, Py_DTSF_ADD_DOT_0)` does.
fn layout(out: &mut String, d: Decimal) {
    let mut buf = StackBuf::new();
    let _ = write!(buf, "{}", d.digits);
    let digits = buf.as_str();
    let n = digits.len() as i32;
    // Position of the decimal point: x = 0.DIGITS * 10**decpt.
    let decpt = d.exp + n;
    if decpt <= -4 || decpt > 16 {
        out.push_str(&digits[..1]);
        if n > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        let exp = decpt - 1;
        out.push('e');
        out.push(if exp < 0 { '-' } else { '+' });
        let _ = write!(out, "{:02}", exp.unsigned_abs());
    } else if decpt <= 0 {
        out.push_str("0.");
        out.extend(std::iter::repeat_n('0', decpt.unsigned_abs() as usize));
        out.push_str(digits);
    } else if decpt >= n {
        out.push_str(digits);
        out.extend(std::iter::repeat_n('0', (decpt - n) as usize));
        out.push_str(".0");
    } else {
        let (int, frac) = digits.split_at(decpt as usize);
        out.push_str(int);
        out.push('.');
        out.push_str(frac);
    }
}

/// Fixed-capacity formatting buffer; the longest `{:e}` of an `f64` is 23 bytes and the
/// longest candidate written by [`shortest`] is under 48.
struct StackBuf {
    bytes: [u8; 48],
    len: usize,
}

impl StackBuf {
    fn new() -> Self {
        StackBuf {
            bytes: [0; 48],
            len: 0,
        }
    }

    fn as_str(&self) -> &str {
        // Only whole `&str`s are ever copied in, so the prefix is valid UTF-8.
        std::str::from_utf8(&self.bytes[..self.len]).unwrap_or_default()
    }
}

impl Write for StackBuf {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let end = self.len + s.len();
        self.bytes
            .get_mut(self.len..end)
            .ok_or(fmt::Error)?
            .copy_from_slice(s.as_bytes());
        self.len = end;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
            (0.00001, "1e-05"),
            (1.5e-5, "1.5e-05"),
            (1e-300, "1e-300"),
            (1e300, "1e+300"),
            (1e100, "1e+100"),
            (5e-324, "5e-324"),
            (f64::MAX, "1.7976931348623157e+308"),
            (f64::MIN_POSITIVE, "2.2250738585072014e-308"),
            (0.30000000000000004, "0.30000000000000004"),
            (2.0f64.powi(53) + 2.0, "9007199254740994.0"),
            (f64::INFINITY, "inf"),
            (f64::NEG_INFINITY, "-inf"),
            (f64::NAN, "nan"),
            (-f64::NAN, "nan"),
        ];
        for &(x, want) in cases {
            assert_eq!(float_repr(x), want, "{x:e}");
        }
    }

    #[test]
    fn f32_layout_rules() {
        let cases: &[(f32, &str)] = &[
            (0.1, "0.1"),
            (-0.0, "-0.0"),
            (1.0 / 3.0, "0.33333334"),
            (16777216.0, "16777216.0"),
            (1e16, "1e+16"),
            (1e-5, "1e-05"),
            (f32::MAX, "3.4028235e+38"),
            (f32::MIN_POSITIVE, "1.1754944e-38"),
            (f32::from_bits(1), "1e-45"),
            (f32::EPSILON, "1.1920929e-07"),
            (f32::NAN, "nan"),
            (f32::NEG_INFINITY, "-inf"),
        ];
        for &(x, want) in cases {
            assert_eq!(f32_repr(x), want, "{x:e}");
        }
    }

    /// `(f64 bits, repr)` lines printed by CPython 3.15.1 for powers of two and ten,
    /// layout boundaries, subnormals, halfway cases and random bit patterns.
    #[test]
    fn matches_cpython_table() {
        let table = include_str!("pyfmt_cases.txt");
        let mut n = 0;
        for line in table.lines().filter(|l| !l.is_empty()) {
            let (bits, want) = line.split_once(' ').unwrap();
            let x = f64::from_bits(u64::from_str_radix(bits, 16).unwrap());
            assert_eq!(float_repr(x), want, "bits {bits}");
            n += 1;
        }
        assert!(n > 3000, "table has {n} cases");
    }

    proptest! {
        #[test]
        fn round_trips(bits in any::<u64>()) {
            let x = f64::from_bits(bits);
            let s = float_repr(x);
            if x.is_nan() {
                prop_assert_eq!(s, "nan");
            } else {
                prop_assert_eq!(s.parse::<f64>().unwrap().to_bits(), x.to_bits());
            }
        }

        #[test]
        fn f32_round_trips(bits in any::<u32>()) {
            let x = f32::from_bits(bits);
            let s = f32_repr(x);
            if x.is_nan() {
                prop_assert_eq!(s, "nan");
            } else {
                prop_assert_eq!(s.parse::<f32>().unwrap().to_bits(), x.to_bits());
            }
        }

        #[test]
        fn normal_values_round_trip(x in any::<f64>()) {
            prop_assert_eq!(float_repr(x).parse::<f64>().unwrap(), x);
        }
    }
}
