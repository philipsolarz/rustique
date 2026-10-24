//! Scalar semantics: Python's operators on fixed-width values. Every function is total (no
//! panics on any input) and reports failure as a flag or [`Fault`], so the slice loops in
//! `exec` can inline them, keep going past bad elements and vectorise.

use std::cmp::Ordering;
use std::fmt;
use std::ops::{Add, Div, Mul, Neg, Rem, Sub};

/// Why one element failed; mapped to an [`ExprError`](super::ExprError) with the context
/// of the failing operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Fault {
    /// The result does not fit the operation's integer type.
    Overflow,
    ZeroDivision,
    /// `0 ** negative`
    ZeroToNegative,
    NegativeShift,
    /// `int ** negative` (a Python `float`, or an error for fixed-width integers).
    NegativeExponent,
    /// `negative ** fractional` (a Python `complex`).
    Complex,
    /// A float `**` overflowed (`OverflowError(ERANGE, ...)`).
    Range,
    NanToInt,
    InfToInt,
    InvalidInt,
    InvalidFloat,
    /// Only Python objects can represent the result.
    Fallback,
}

/// `bool`, integer and float element types, read as numbers (`bool` as 0/1).
pub(super) trait Num: Copy + Default + PartialOrd + Send + Sync + 'static {
    /// Exact for `bool` and integers (floats saturate; never used for them).
    fn to_i128(self) -> i128;
    /// Correctly rounded.
    fn to_f64(self) -> f64;
    /// Correctly rounded.
    fn to_f32(self) -> f32;
    /// Integers: `None` when out of range. Floats: correctly rounded.
    fn from_i128(v: i128) -> Option<Self>;
    /// Convert a number of another type (integer targets are range-checked; float sources
    /// are never converted to integers through this).
    fn from_num<S: Num>(v: S) -> Option<Self>;
    fn nonzero(self) -> bool;
}

macro_rules! impl_num_int {
    ($($t:ty),* $(,)?) => {$(
        impl Num for $t {
            #[inline(always)]
            fn to_i128(self) -> i128 { self as i128 }
            #[inline(always)]
            fn to_f64(self) -> f64 { self as f64 }
            #[inline(always)]
            fn to_f32(self) -> f32 { self as f32 }
            #[inline(always)]
            fn from_i128(v: i128) -> Option<Self> { <$t>::try_from(v).ok() }
            #[inline(always)]
            fn from_num<S: Num>(v: S) -> Option<Self> { Self::from_i128(v.to_i128()) }
            #[inline(always)]
            fn nonzero(self) -> bool { self != 0 }
        }
    )*};
}

impl_num_int!(i8, i16, i32, i64, u8, u16, u32, u64);

impl Num for bool {
    #[inline(always)]
    fn to_i128(self) -> i128 {
        self as i128
    }
    #[inline(always)]
    fn to_f64(self) -> f64 {
        self as u8 as f64
    }
    #[inline(always)]
    fn to_f32(self) -> f32 {
        self as u8 as f32
    }
    #[inline(always)]
    fn from_i128(v: i128) -> Option<Self> {
        match v {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        }
    }
    #[inline(always)]
    fn from_num<S: Num>(v: S) -> Option<Self> {
        Self::from_i128(v.to_i128())
    }
    #[inline(always)]
    fn nonzero(self) -> bool {
        self
    }
}

impl Num for f64 {
    #[inline(always)]
    fn to_i128(self) -> i128 {
        self as i128
    }
    #[inline(always)]
    fn to_f64(self) -> f64 {
        self
    }
    #[inline(always)]
    fn to_f32(self) -> f32 {
        self as f32
    }
    #[inline(always)]
    fn from_i128(v: i128) -> Option<Self> {
        Some(v as f64)
    }
    #[inline(always)]
    fn from_num<S: Num>(v: S) -> Option<Self> {
        Some(v.to_f64())
    }
    #[inline(always)]
    fn nonzero(self) -> bool {
        self != 0.0
    }
}

impl Num for f32 {
    #[inline(always)]
    fn to_i128(self) -> i128 {
        self as i128
    }
    #[inline(always)]
    fn to_f64(self) -> f64 {
        self as f64
    }
    #[inline(always)]
    fn to_f32(self) -> f32 {
        self
    }
    #[inline(always)]
    fn from_i128(v: i128) -> Option<Self> {
        Some(v as f32)
    }
    #[inline(always)]
    fn from_num<S: Num>(v: S) -> Option<Self> {
        Some(v.to_f32())
    }
    #[inline(always)]
    fn nonzero(self) -> bool {
        self != 0.0
    }
}

/// Python's integer operators on a fixed-width integer type. Each returns
/// `(result, failed)`; failed results are garbage but never panic.
pub(super) trait PyInt: Num + Ord + Eq + fmt::Display {
    const ZERO: Self;
    const ONE: Self;
    const MIN: Self;
    const MAX: Self;
    fn wrapping_add(self, b: Self) -> Self;
    fn wrapping_sub(self, b: Self) -> Self;
    fn wrapping_mul(self, b: Self) -> Self;
    fn overflowing_add(self, b: Self) -> (Self, bool);
    fn overflowing_sub(self, b: Self) -> (Self, bool);
    fn overflowing_mul(self, b: Self) -> (Self, bool);
    fn checked_mul(self, b: Self) -> Option<Self>;
    /// `a // b`; fails on division by zero and overflow (`MIN // -1`).
    fn py_floordiv(self, b: Self) -> (Self, bool);
    /// `a % b` (sign of the divisor); fails on division by zero.
    fn py_mod(self, b: Self) -> (Self, bool);
    /// `a ** e`; fails for `e < 0` and on overflow.
    fn py_pow(self, e: Self) -> (Self, bool);
    /// `a << b`; fails for `b < 0` and when bits would be lost.
    fn py_shl(self, b: Self) -> (Self, bool);
    /// `a >> b` (floor); fails for `b < 0`.
    fn py_shr(self, b: Self) -> (Self, bool);
    /// `-a`; fails when the negation does not fit (unsigned: unless zero).
    fn py_neg(self) -> (Self, bool);
    /// `abs(a)`.
    fn py_abs(self) -> (Self, bool);
    fn not(self) -> Self;
    fn and(self, b: Self) -> Self;
    fn or(self, b: Self) -> Self;
    fn xor(self, b: Self) -> Self;
    fn is_negative(self) -> bool;
    /// `|self| <= 2**53`, i.e. converting to `f64` is exact.
    fn exact_f64(self) -> bool;
}

macro_rules! impl_py_int_common {
    ($t:ty) => {
        const ZERO: Self = 0;
        const ONE: Self = 1;
        const MIN: Self = <$t>::MIN;
        const MAX: Self = <$t>::MAX;
        #[inline(always)]
        fn wrapping_add(self, b: Self) -> Self { self.wrapping_add(b) }
        #[inline(always)]
        fn wrapping_sub(self, b: Self) -> Self { self.wrapping_sub(b) }
        #[inline(always)]
        fn wrapping_mul(self, b: Self) -> Self { self.wrapping_mul(b) }
        #[inline(always)]
        fn overflowing_add(self, b: Self) -> (Self, bool) { self.overflowing_add(b) }
        #[inline(always)]
        fn overflowing_sub(self, b: Self) -> (Self, bool) { self.overflowing_sub(b) }
        #[inline(always)]
        fn overflowing_mul(self, b: Self) -> (Self, bool) { self.overflowing_mul(b) }
        #[inline(always)]
        fn checked_mul(self, b: Self) -> Option<Self> { self.checked_mul(b) }
        #[inline(always)]
        fn not(self) -> Self { !self }
        #[inline(always)]
        fn and(self, b: Self) -> Self { self & b }
        #[inline(always)]
        fn or(self, b: Self) -> Self { self | b }
        #[inline(always)]
        fn xor(self, b: Self) -> Self { self ^ b }
        #[inline]
        fn py_pow(self, e: Self) -> (Self, bool) {
            if e < 0 as Self {
                return (0, true);
            }
            pow_unsigned_exponent(self, e as u64)
        }
        #[inline]
        fn py_shl(self, b: Self) -> (Self, bool) {
            // Clamp the shift amount into range first so the shift itself can never be an
            // out-of-range (masked) shift; overflow is detected separately.
            let s = b.clamp(0 as Self, Self::BITS as Self - 1) as u32;
            let r = self << s;
            let lost = (r >> s) != self || (b >= Self::BITS as Self && self != 0);
            (r, b < 0 as Self || lost)
        }
    };
}

macro_rules! impl_py_int_signed {
    ($($t:ty),*) => {$(
        impl PyInt for $t {
            impl_py_int_common!($t);
            #[inline(always)]
            fn py_floordiv(self, b: Self) -> (Self, bool) {
                let bad = b == 0 || (self == Self::MIN && b == -1);
                let d = if bad { 1 } else { b };
                let q = self / d;
                let r = self % d;
                (if r != 0 && ((r < 0) != (d < 0)) { q - 1 } else { q }, bad)
            }
            #[inline(always)]
            fn py_mod(self, b: Self) -> (Self, bool) {
                let bad = b == 0;
                let d = if bad { 1 } else { b };
                let r = self.wrapping_rem(d);
                (if r != 0 && ((r < 0) != (d < 0)) { r + d } else { r }, bad)
            }
            #[inline(always)]
            fn py_shr(self, b: Self) -> (Self, bool) {
                // Shifting by BITS - 1 already gives the floor (0 or -1).
                (self >> b.clamp(0, Self::BITS as Self - 1) as u32, b < 0)
            }
            #[inline(always)]
            fn py_neg(self) -> (Self, bool) { self.overflowing_neg() }
            #[inline(always)]
            fn py_abs(self) -> (Self, bool) { self.overflowing_abs() }
            #[inline(always)]
            fn is_negative(self) -> bool { self < 0 }
            #[inline(always)]
            fn exact_f64(self) -> bool { (self as i128).unsigned_abs() <= 1 << 53 }
        }
    )*};
}

macro_rules! impl_py_int_unsigned {
    ($($t:ty),*) => {$(
        impl PyInt for $t {
            impl_py_int_common!($t);
            #[inline(always)]
            fn py_floordiv(self, b: Self) -> (Self, bool) {
                let d = if b == 0 { 1 } else { b };
                (self / d, b == 0)
            }
            #[inline(always)]
            fn py_mod(self, b: Self) -> (Self, bool) {
                let d = if b == 0 { 1 } else { b };
                (self % d, b == 0)
            }
            #[inline(always)]
            fn py_shr(self, b: Self) -> (Self, bool) {
                // For b >= BITS, the extra shift by one clears the last bit.
                let s = b.min(Self::BITS as Self - 1) as u32;
                ((self >> s) >> u32::from(b >= Self::BITS as Self), false)
            }
            #[inline(always)]
            fn py_neg(self) -> (Self, bool) { (self.wrapping_neg(), self != 0) }
            #[inline(always)]
            fn py_abs(self) -> (Self, bool) { (self, false) }
            #[inline(always)]
            fn is_negative(self) -> bool { false }
            #[inline(always)]
            fn exact_f64(self) -> bool { (self as u128) <= 1 << 53 }
        }
    )*};
}

impl_py_int_signed!(i8, i16, i32, i64);
impl_py_int_unsigned!(u8, u16, u32, u64);

/// Exponentiation by squaring; at most 64 rounds, failing on the first overflow.
#[inline]
fn pow_unsigned_exponent<T: PyInt>(base: T, mut e: u64) -> (T, bool) {
    let mut acc = T::ONE;
    let mut b = base;
    loop {
        if e & 1 == 1 {
            match acc.checked_mul(b) {
                Some(v) => acc = v,
                None => return (T::ZERO, true),
            }
        }
        e >>= 1;
        if e == 0 {
            return (acc, false);
        }
        // A remaining set bit multiplies `acc` by at least `b * b`, so overflow here is real.
        match b.checked_mul(b) {
            Some(v) => b = v,
            None => return (T::ZERO, true),
        }
    }
}

/// Inclusive range of `x` for which `f(x)` (an operation with a constant) stays in range,
/// computed once per kernel call so the loop only does a wrapping op and two compares.
pub(super) struct Bounds<T> {
    pub lo: T,
    pub hi: T,
}

impl<T: PyInt> Bounds<T> {
    fn clamp(lo: i128, hi: i128) -> Self {
        let (min, max) = (T::MIN.to_i128(), T::MAX.to_i128());
        let (lo, hi) = (lo.max(min), hi.min(max));
        if lo > hi {
            // Nothing fits: an empty range rejects every x.
            return Bounds { lo: T::MAX, hi: T::MIN };
        }
        Bounds { lo: T::from_i128(lo).unwrap_or(T::MAX), hi: T::from_i128(hi).unwrap_or(T::MIN) }
    }

    /// `x + c`
    pub fn add(c: T) -> Self {
        let c = c.to_i128();
        Self::clamp(T::MIN.to_i128() - c, T::MAX.to_i128() - c)
    }

    /// `x - c`
    pub fn sub(c: T) -> Self {
        let c = c.to_i128();
        Self::clamp(T::MIN.to_i128() + c, T::MAX.to_i128() + c)
    }

    /// `c - x`
    pub fn rsub(c: T) -> Self {
        let c = c.to_i128();
        Self::clamp(c - T::MAX.to_i128(), c - T::MIN.to_i128())
    }

    /// `x * c`
    pub fn mul(c: T) -> Self {
        let (min, max, c) = (T::MIN.to_i128(), T::MAX.to_i128(), c.to_i128());
        match c.cmp(&0) {
            Ordering::Equal => Self::clamp(min, max),
            Ordering::Greater => Self::clamp(div_ceil(min, c), div_floor(max, c)),
            Ordering::Less => Self::clamp(div_ceil(max, c), div_floor(min, c)),
        }
    }

    #[inline(always)]
    pub fn outside(&self, x: T) -> bool {
        x < self.lo || x > self.hi
    }
}

fn div_floor(a: i128, b: i128) -> i128 {
    let q = a / b;
    if a % b != 0 && ((a < 0) != (b < 0)) { q - 1 } else { q }
}

fn div_ceil(a: i128, b: i128) -> i128 {
    -div_floor(-a, b)
}

/// Python's `int / int`: the correctly rounded quotient. Fails on division by zero.
#[inline(always)]
pub(super) fn int_truediv<T: PyInt>(a: T, b: T) -> (f64, bool) {
    if b == T::ZERO {
        return (0.0, true);
    }
    if a.exact_f64() && b.exact_f64() {
        // Both conversions are exact and IEEE division rounds correctly.
        return (a.to_f64() / b.to_f64(), false);
    }
    (exact_quotient(a.to_i128(), b.to_i128()), false)
}

/// Correctly rounded `a / b` for `|a|, |b| <= 2**64`, `b != 0`.
fn exact_quotient(a: i128, b: i128) -> f64 {
    let (n, d) = (a.unsigned_abs(), b.unsigned_abs());
    let magnitude = if n == 0 {
        0.0
    } else {
        let bits = |x: u128| 128 - x.leading_zeros() as i64;
        // Scale so the integer quotient has at least 55 significant bits; the remainder
        // then only matters as a sticky bit below the rounding position.
        let shift = (55 + bits(d) - bits(n)).max(0) as u64;
        let scaled = n << shift;
        let q = (scaled / d) | u128::from(!scaled.is_multiple_of(d));
        // 2**-shift, exact (shift <= 119).
        (q as f64) * f64::from_bits((1023 - shift) << 52)
    };
    if (a < 0) != (b < 0) { -magnitude } else { magnitude }
}

/// Exact comparison of an integer (or bool) with a float, like Python's `int < float`.
#[inline(always)]
pub(super) fn int_float_cmp<I: Num>(a: I, b: f64) -> Option<Ordering> {
    let fa = a.to_f64();
    // Rounding is monotonic, so a strict float comparison is exact; on a tie `b` is an
    // integer within the integer type's range (or 2**64) and fits an i128.
    if fa < b {
        Some(Ordering::Less)
    } else if fa > b {
        Some(Ordering::Greater)
    } else if fa == b {
        Some(a.to_i128().cmp(&(b as i128)))
    } else {
        None
    }
}

/// `int(x)` (or a fixed-width integer conversion) of a float: truncates.
#[inline(always)]
pub(super) fn float_to_int<D: Num>(x: f64) -> Result<D, Fault> {
    if x.is_nan() {
        return Err(Fault::NanToInt);
    }
    if x.is_infinite() {
        return Err(Fault::InfToInt);
    }
    // Saturates beyond i128, which is out of range for every target.
    D::from_i128(x.trunc() as i128).ok_or(Fault::Overflow)
}

/// Floating-point types with Python's float operator semantics.
pub(super) trait PyFloat:
    Num
    + fmt::Debug
    + Neg<Output = Self>
    + Add<Output = Self>
    + Sub<Output = Self>
    + Mul<Output = Self>
    + Div<Output = Self>
    + Rem<Output = Self>
{
    const ZERO: Self;
    const ONE: Self;
    const TWO: Self;
    const HALF: Self;
    fn floor(self) -> Self;
    fn abs(self) -> Self;
    fn copysign(self, sign: Self) -> Self;
    fn is_nan(self) -> bool;
    fn is_infinite(self) -> bool;
    fn powf(self, e: Self) -> Self;
}

macro_rules! impl_py_float {
    ($($t:ty),*) => {$(
        impl PyFloat for $t {
            const ZERO: Self = 0.0;
            const ONE: Self = 1.0;
            const TWO: Self = 2.0;
            const HALF: Self = 0.5;
            #[inline(always)]
            fn floor(self) -> Self { self.floor() }
            #[inline(always)]
            fn abs(self) -> Self { self.abs() }
            #[inline(always)]
            fn copysign(self, sign: Self) -> Self { self.copysign(sign) }
            #[inline(always)]
            fn is_nan(self) -> bool { self.is_nan() }
            #[inline(always)]
            fn is_infinite(self) -> bool { self.is_infinite() }
            #[inline(always)]
            fn powf(self, e: Self) -> Self { self.powf(e) }
        }
    )*};
}

impl_py_float!(f32, f64);

/// CPython's `float.__mod__` for `w != 0` (Rust's `%` is C's `fmod`).
#[inline(always)]
pub(super) fn float_mod<F: PyFloat>(v: F, w: F) -> F {
    let m = v % w;
    if m != F::ZERO {
        if (w < F::ZERO) != (m < F::ZERO) { m + w } else { m }
    } else {
        F::ZERO.copysign(w)
    }
}

/// CPython's `float.__floordiv__` for `w != 0` (`_float_div_mod`).
#[inline(always)]
pub(super) fn float_floordiv<F: PyFloat>(v: F, w: F) -> F {
    let m = v % w;
    let mut div = (v - m) / w;
    if m != F::ZERO && ((w < F::ZERO) != (m < F::ZERO)) {
        div = div - F::ONE;
    }
    if div != F::ZERO {
        let f = div.floor();
        if div - f > F::HALF { f + F::ONE } else { f }
    } else {
        F::ZERO.copysign(v / w)
    }
}

fn is_odd_integer<F: PyFloat>(x: F) -> bool {
    x.abs() % F::TWO == F::ONE
}

/// CPython's `float.__pow__`, including its special cases.
#[inline]
pub(super) fn float_pow<F: PyFloat>(v: F, w: F) -> Result<F, Fault> {
    if w == F::ZERO {
        return Ok(F::ONE);
    }
    if v.is_nan() {
        return Ok(v);
    }
    if w.is_nan() {
        return Ok(if v == F::ONE { F::ONE } else { w });
    }
    if w.is_infinite() {
        let a = v.abs();
        return Ok(if a == F::ONE {
            F::ONE
        } else if (w > F::ZERO) == (a > F::ONE) {
            w.abs()
        } else {
            F::ZERO
        });
    }
    if v.is_infinite() {
        let odd = is_odd_integer(w);
        return Ok(if w > F::ZERO {
            if odd { v } else { v.abs() }
        } else if odd {
            F::ZERO.copysign(v)
        } else {
            F::ZERO
        });
    }
    if v == F::ZERO {
        if w < F::ZERO {
            return Err(Fault::ZeroToNegative);
        }
        return Ok(if is_odd_integer(w) { v } else { F::ZERO });
    }
    let mut base = v;
    let mut negate = false;
    if v < F::ZERO {
        if w != w.floor() {
            return Err(Fault::Complex);
        }
        base = -v;
        negate = is_odd_integer(w);
    }
    if base == F::ONE {
        return Ok(if negate { -F::ONE } else { F::ONE });
    }
    let r = base.powf(w);
    if r.is_infinite() {
        return Err(Fault::Range);
    }
    Ok(if negate { -r } else { r })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn int_floor_semantics() {
        assert_eq!((-7i64).py_floordiv(2), (-4, false));
        assert_eq!(7i64.py_floordiv(-2), (-4, false));
        assert_eq!((-7i64).py_mod(2), (1, false));
        assert_eq!(7i64.py_mod(-2), (-1, false));
        assert!(i64::MIN.py_floordiv(-1).1);
        assert_eq!(i64::MIN.py_mod(-1), (0, false));
        assert!(5i64.py_floordiv(0).1 && 5i64.py_mod(0).1);
        assert_eq!(7u8.py_floordiv(2), (3, false));
    }

    #[test]
    fn int_pow_and_shifts() {
        assert_eq!(2i64.py_pow(62), (1 << 62, false));
        assert!(2i64.py_pow(63).1);
        assert_eq!((-2i64).py_pow(63), (i64::MIN, false));
        assert_eq!((-1i64).py_pow(i64::MAX), (-1, false));
        assert_eq!(0i64.py_pow(0), (1, false));
        assert!(2i8.py_pow(-1).1);
        assert_eq!(1i8.py_shl(6), (64, false));
        assert!(1i8.py_shl(7).1);
        assert_eq!(0i64.py_shl(1000), (0, false));
        assert!(1i64.py_shl(-1).1);
        assert_eq!((-5i64).py_shr(1000), (-1, false));
        assert_eq!(200u8.py_shr(9), (0, false));
        assert_eq!((-5i64).py_shr(1), (-3, false));
        assert!(1u8.py_neg().1);
        assert!(i8::MIN.py_abs().1);
        // Opaque operands so the out-of-range shift paths are exercised at run time
        // rather than constant-folded.
        use std::hint::black_box;
        assert_eq!(black_box(1i64 << 40).py_shr(black_box(100)), (0, false));
        assert_eq!(black_box(-(1i64 << 40)).py_shr(black_box(100)), (-1, false));
        assert_eq!(black_box(1u64 << 40).py_shr(black_box(100)), (0, false));
        assert_eq!(black_box(255u8).py_shr(black_box(8)), (0, false));
        assert!(black_box(1i64).py_shl(black_box(100)).1);
        assert_eq!(black_box(0i64).py_shl(black_box(100)), (0, false));
    }

    #[test]
    fn bounds_match_checked_ops() {
        for c in [i8::MIN, -100, -2, -1, 0, 1, 2, 3, 100, i8::MAX] {
            let (add, sub, rsub, mul) = (Bounds::add(c), Bounds::sub(c), Bounds::rsub(c), Bounds::mul(c));
            for x in i8::MIN..=i8::MAX {
                assert_eq!(add.outside(x), x.checked_add(c).is_none());
                assert_eq!(sub.outside(x), x.checked_sub(c).is_none());
                assert_eq!(rsub.outside(x), c.checked_sub(x).is_none());
                assert_eq!(mul.outside(x), x.checked_mul(c).is_none(), "{x} * {c}");
            }
        }
        for c in [0u8, 1, 2, 200, 255] {
            let (rsub, mul) = (Bounds::rsub(c), Bounds::mul(c));
            for x in 0..=255u8 {
                assert_eq!(rsub.outside(x), c.checked_sub(x).is_none());
                assert_eq!(mul.outside(x), x.checked_mul(c).is_none());
            }
        }
    }

    #[test]
    fn true_division_is_correctly_rounded() {
        // Expected values from CPython 3.15.
        assert_eq!(int_truediv(i64::MAX, 3), (3.0744573456182584e18, false));
        assert_eq!(int_truediv(9007199254740993i64, 1), (9007199254740992.0, false));
        assert_eq!(int_truediv(9007199254740995i64, 1), (9007199254740996.0, false));
        assert_eq!(int_truediv(i64::MIN, -1), (9.223372036854776e18, false));
        assert_eq!(int_truediv(1i64, i64::MIN), (-1.0842021724855044e-19, false));
        assert_eq!(int_truediv(u64::MAX, 7), (2.6352491533870787e18, false));
        assert!(int_truediv(1i64, 0).1);
        assert!(int_truediv(0i64, -5).0.is_sign_negative());
    }

    #[test]
    fn exact_int_float_comparison() {
        let big = 9007199254740993i64; // 2**53 + 1 rounds to 2**53 as a float
        assert_eq!(int_float_cmp(big, 9007199254740992.0), Some(Ordering::Greater));
        assert_eq!(int_float_cmp(i64::MAX, 9.223_372_036_854_776e18), Some(Ordering::Less));
        assert_eq!(int_float_cmp(u64::MAX, 1.8446744073709552e19), Some(Ordering::Less));
        assert_eq!(int_float_cmp(3i64, 2.5), Some(Ordering::Greater));
        assert_eq!(int_float_cmp(3i64, f64::NAN), None);
        assert_eq!(int_float_cmp(true, 1.0), Some(Ordering::Equal));
    }

    #[test]
    fn float_semantics() {
        assert_eq!(float_floordiv(7.0, -2.0), -4.0);
        assert_eq!(float_mod(7.0, -2.0), -1.0);
        assert_eq!(float_mod(-0.0f64, 5.0).to_bits(), 0.0f64.to_bits());
        assert_eq!(float_floordiv(2.5, f64::INFINITY), 0.0);
        assert_eq!(float_floordiv(-2.5, f64::INFINITY), -1.0);
        assert!(float_floordiv(f64::INFINITY, 1.0).is_nan());
        assert_eq!(float_mod(-2.5, f64::INFINITY), f64::INFINITY);
        assert_eq!(float_pow(0.0, -1.0), Err(Fault::ZeroToNegative));
        assert_eq!(float_pow(-8.0, 1.0 / 3.0), Err(Fault::Complex));
        assert_eq!(float_pow(2.0, 10000.0), Err(Fault::Range));
        assert_eq!(float_pow(-2.0, 3.0), Ok(-8.0));
        assert_eq!(float_pow(-1.0, 1e300), Ok(1.0));
        assert_eq!(float_pow(1.0, f64::NAN), Ok(1.0));
        assert_eq!(float_pow(f64::NAN, 0.0), Ok(1.0));
        assert_eq!(float_pow(-0.0, 3.0).map(f64::to_bits), Ok((-0.0f64).to_bits()));
        assert_eq!(float_pow(f64::NEG_INFINITY, -3.0).map(f64::to_bits), Ok((-0.0f64).to_bits()));
        assert_eq!(float_pow(0.5, f64::NEG_INFINITY), Ok(f64::INFINITY));
        assert_eq!(float_pow(2.0, -1075.0), Ok(0.0));
    }

    #[test]
    fn float_to_int_conversion() {
        assert_eq!(float_to_int::<i64>(-2.9), Ok(-2));
        assert_eq!(float_to_int::<i64>(9.2e18), Ok(9_200_000_000_000_000_000));
        assert_eq!(float_to_int::<i64>(9.3e18), Err(Fault::Overflow));
        assert_eq!(float_to_int::<u8>(255.9), Ok(255));
        assert_eq!(float_to_int::<u8>(-0.5), Ok(0));
        assert_eq!(float_to_int::<i8>(f64::NAN), Err(Fault::NanToInt));
        assert_eq!(float_to_int::<i8>(f64::NEG_INFINITY), Err(Fault::InfToInt));
    }
}
