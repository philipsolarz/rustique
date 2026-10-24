//! Bulk operations on columns, with Python semantics.
//!
//! OWNER: kernels agent. The signatures below are the INTERFACE used by the bindings.
//!
//! Every kernel dispatches once on the column variant and then runs a tight typed loop; no
//! per-element [`Value`]s are built. Numbers behave like Python's: `bool` and the integer
//! dtypes are exact integers, `f32`/`f64` are Python floats (`f32` widens exactly), and an
//! integer compares with a float *exactly* (`2**53 + 1 != float(2**53)`). `str` only equals
//! `str` and orders by code point (UTF-8 byte order).
//!
//! Native float storage behaves as if each NaN were one shared object (ARCHITECTURE §4.1):
//! NaN equals NaN in [`find`], [`count`], [`equal`] and in the equal-prefix scan of
//! [`lex_compare`]. The scalar helpers [`value_eq`]/[`value_cmp`] use plain IEEE semantics.

use std::cmp::Ordering;
use std::fmt;

use rayon::prelude::*;

use crate::column::{Column, Element, Value};
use crate::dtype::DType;

/// Result of [`sum`]: integer columns (and bool) sum exactly into `i128`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Sum {
    Int(i128),
    Float(f64),
}

/// Ordering operator for [`lex_compare`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CmpOp {
    Lt,
    Le,
    Gt,
    Ge,
}

impl CmpOp {
    /// Whether `a <op> b` holds, given `a.partial_cmp(b)`; unordered (`None`, a NaN) satisfies
    /// no operator, as in IEEE 754 and Python.
    pub fn holds(self, ordering: Option<Ordering>) -> bool {
        use Ordering::{Equal, Greater, Less};
        matches!(
            (self, ordering),
            (CmpOp::Lt, Some(Less))
                | (CmpOp::Le, Some(Less | Equal))
                | (CmpOp::Gt, Some(Greater))
                | (CmpOp::Ge, Some(Greater | Equal))
        )
    }

    /// The Python operator symbol (`"<"`, …), for error messages.
    pub const fn symbol(self) -> &'static str {
        match self {
            CmpOp::Lt => "<",
            CmpOp::Le => "<=",
            CmpOp::Gt => ">",
            CmpOp::Ge => ">=",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KernelError {
    /// The operation is not defined for this dtype (e.g. `sum` of strings).
    Unsupported { op: &'static str, dtype: DType },
}

impl fmt::Display for KernelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KernelError::Unsupported { op, dtype } => write!(f, "{op} is not supported for {dtype} data"),
        }
    }
}

impl std::error::Error for KernelError {}

// Thresholds measured with `examples/kernels_bench.rs tune` (8 threads): below them the
// rayon fork/join overhead outweighs the gain.

/// Inputs at least this long run sums, `count` and `min`/`max` on the rayon pool.
const PAR_SCAN_MIN: usize = 1 << 18;
/// Inputs at least this long sort on the rayon pool.
const PAR_SORT_MIN: usize = 1 << 14;
/// 16-bit columns at least this long use a counting sort.
const COUNTING_SORT_16_MIN: usize = 1 << 13;
/// Block length of the blocked scans; also bounds the per-block accumulators of `sum_wide`.
const SUM_BLOCK: usize = 1 << 16;

// ---------------------------------------------------------------------------------------
// Python's numeric model
// ---------------------------------------------------------------------------------------

/// A number as Python sees it. `Int` always holds a value of at most 64 bits.
#[derive(Clone, Copy, Debug)]
enum Num {
    Int(i128),
    Float(f64),
}

const TWO127: f64 = 170_141_183_460_469_231_731_687_303_715_884_105_728.0;

/// `f` as an integer if it is integral and within `±2^127` (never NaN or infinite).
fn float_to_int(f: f64) -> Option<i128> {
    (f.fract() == 0.0 && f.abs() < TWO127).then_some(f as i128)
}

/// Exact comparison of an integer with a float (`None` for NaN), as Python's `float_richcompare`.
fn int_float_cmp(i: i128, f: f64) -> Option<Ordering> {
    if f.is_nan() {
        return None;
    }
    if f >= TWO127 {
        return Some(Ordering::Less);
    }
    if f < -TWO127 {
        return Some(Ordering::Greater);
    }
    let whole = f.trunc();
    let frac = f - whole;
    Some(i.cmp(&(whole as i128)).then(if frac > 0.0 {
        Ordering::Less
    } else if frac < 0.0 {
        Ordering::Greater
    } else {
        Ordering::Equal
    }))
}

/// Equality for column kernels: Python `==`, except that NaN equals NaN. The `Num` matches
/// fold away per type pair.
#[inline]
fn elem_eq<A: Numeric, B: Numeric>(a: A, b: B) -> bool {
    match (a.num(), b.num()) {
        (Num::Int(x), Num::Int(y)) => x == y,
        (Num::Int(_), Num::Float(f)) => a.eq_f64(f),
        (Num::Float(f), Num::Int(_)) => b.eq_f64(f),
        (Num::Float(x), Num::Float(y)) => x == y || (x.is_nan() && y.is_nan()),
    }
}

/// Python ordering of two numbers (`None` when a NaN is involved).
fn num_cmp(a: Num, b: Num) -> Option<Ordering> {
    match (a, b) {
        (Num::Int(x), Num::Int(y)) => Some(x.cmp(&y)),
        (Num::Int(i), Num::Float(f)) => int_float_cmp(i, f),
        (Num::Float(f), Num::Int(i)) => int_float_cmp(i, f).map(Ordering::reverse),
        (Num::Float(x), Num::Float(y)) => x.partial_cmp(&y),
    }
}

fn value_num(value: &Value) -> Option<Num> {
    match *value {
        Value::F32(x) => Some(Num::Float(x as f64)),
        Value::F64(x) => Some(Num::Float(x)),
        Value::Str(_) => None,
        ref int => int.as_i128().map(Num::Int),
    }
}

/// The numeric element types (everything but `str`).
trait Numeric: Element + Copy + PartialOrd {
    const FLOAT: bool = false;
    /// Width of [`Numeric::key`] in bits.
    const KEY_BITS: u32;
    fn num(self) -> Num;
    /// Exactly Python's `self == f` (so never true for NaN).
    fn eq_f64(self, f: f64) -> bool;
    /// The element that equals `n` in Python, if any (`None` for NaN).
    fn from_num(n: Num) -> Option<Self>;
    /// Order-preserving unsigned key: total for integers; for floats valid for non-NaN
    /// values, with `-0.0` and `0.0` mapping to the same key.
    fn key(self) -> u64;
    #[inline]
    fn is_nan(self) -> bool {
        false
    }
    /// `±0.0` (the only distinct floats that compare equal); always false for integers.
    #[inline]
    fn is_float_zero(self) -> bool {
        false
    }
    #[inline]
    fn is_sign_negative(self) -> bool {
        false
    }
}

macro_rules! impl_int {
    ($($t:ty => $u:ty),* $(,)?) => {$(
        impl Numeric for $t {
            const KEY_BITS: u32 = <$t>::BITS;
            #[inline]
            fn num(self) -> Num {
                Num::Int(self as i128)
            }
            #[inline]
            fn eq_f64(self, f: f64) -> bool {
                // Narrow ints convert exactly. A float equal to a rounded 64-bit int is
                // integral, so casting it back is exact, except where the cast saturates.
                if <$t>::BITS < 64 {
                    self as f64 == f
                } else {
                    f == self as f64 && f != <$t>::MAX as f64 && f as $t == self
                }
            }
            #[inline]
            fn from_num(n: Num) -> Option<Self> {
                let i = match n {
                    Num::Int(i) => i,
                    Num::Float(f) => float_to_int(f)?,
                };
                <$t>::try_from(i).ok()
            }
            #[inline]
            fn key(self) -> u64 {
                (self as $u ^ <$t>::MIN as $u) as u64
            }
        }
    )*};
}

impl_int!(i8 => u8, i16 => u16, i32 => u32, i64 => u64, u8 => u8, u16 => u16, u32 => u32, u64 => u64);

impl Numeric for bool {
    const KEY_BITS: u32 = 1;
    #[inline]
    fn num(self) -> Num {
        Num::Int(self as i128)
    }
    #[inline]
    fn eq_f64(self, f: f64) -> bool {
        self as u8 as f64 == f
    }
    fn from_num(n: Num) -> Option<Self> {
        match n {
            Num::Int(0) => Some(false),
            Num::Int(1) => Some(true),
            Num::Int(_) => None,
            Num::Float(f) => Self::from_num(Num::Int(float_to_int(f)?)),
        }
    }
    #[inline]
    fn key(self) -> u64 {
        self as u64
    }
}

macro_rules! impl_float {
    ($($t:ty => $bits:ty, $signed:ty);* $(;)?) => {$(
        impl Numeric for $t {
            const FLOAT: bool = true;
            const KEY_BITS: u32 = <$bits>::BITS;
            #[inline]
            fn num(self) -> Num {
                Num::Float(self as f64)
            }
            #[inline]
            fn eq_f64(self, f: f64) -> bool {
                self as f64 == f
            }
            fn from_num(n: Num) -> Option<Self> {
                match n {
                    Num::Int(i) => {
                        let x = i as $t;
                        (float_to_int(x as f64) == Some(i)).then_some(x)
                    }
                    Num::Float(f) => {
                        let x = f as $t;
                        (x as f64 == f).then_some(x)
                    }
                }
            }
            #[inline]
            fn key(self) -> u64 {
                // `+ 0.0` turns -0.0 into 0.0. Negative floats flip all bits, others only the
                // sign bit, mapping IEEE order onto unsigned order (branch-free).
                let bits = (self + 0.0).to_bits();
                let negative = (bits as $signed >> (<$bits>::BITS - 1)) as $bits;
                (bits ^ (negative | 1 << (<$bits>::BITS - 1))) as u64
            }
            #[inline]
            fn is_nan(self) -> bool {
                <$t>::is_nan(self)
            }
            #[inline]
            fn is_float_zero(self) -> bool {
                self == 0.0
            }
            #[inline]
            fn is_sign_negative(self) -> bool {
                <$t>::is_sign_negative(self)
            }
        }
    )*};
}

impl_float!(f32 => u32, i32; f64 => u64, i64);

/// Dispatch over the numeric variants (binding the `Vec` to `$v`), with a separate `Str` arm.
macro_rules! numeric {
    ($col:expr, $v:ident => $body:expr, $s:ident => $str:expr) => {
        match $col {
            Column::Bool($v) => $body,
            Column::I8($v) => $body,
            Column::I16($v) => $body,
            Column::I32($v) => $body,
            Column::I64($v) => $body,
            Column::U8($v) => $body,
            Column::U16($v) => $body,
            Column::U32($v) => $body,
            Column::U64($v) => $body,
            Column::F32($v) => $body,
            Column::F64($v) => $body,
            Column::Str($s) => $str,
        }
    };
}

/// Index of the first element satisfying `pred`. Whole blocks are tested without branching
/// so the scan vectorizes; only the hit block is searched element by element.
#[inline]
fn position<T: Copy>(v: &[T], pred: impl Fn(T) -> bool) -> Option<usize> {
    const BLOCK: usize = 32;
    for (b, block) in v.chunks(BLOCK).enumerate() {
        if block.iter().fold(false, |hit, &x| hit | pred(x)) {
            return block.iter().position(|&x| pred(x)).map(|i| b * BLOCK + i);
        }
    }
    None
}

fn count_where<T: Copy + Sync>(v: &[T], pred: impl Fn(T) -> bool + Sync) -> usize {
    let count = |block: &[T]| block.iter().filter(|&&x| pred(x)).count();
    if v.len() >= PAR_SCAN_MIN { v.par_chunks(SUM_BLOCK).map(count).sum() } else { count(v) }
}

// ---------------------------------------------------------------------------------------
// sum
// ---------------------------------------------------------------------------------------

/// `sum(column)`; bool counts as int. Strings are unsupported.
///
/// Matches Python's `sum(list)` exactly: integers sum exactly, floats reproduce CPython's
/// compensated summation bit for bit, and an empty column sums to the int `0` (whatever its
/// dtype, so an empty `str` column is fine too).
pub fn sum(column: &Column) -> Result<Sum, KernelError> {
    Ok(match column {
        Column::Bool(v) => Sum::Int(count_where(v, |b| b) as i128),
        Column::I8(v) => Sum::Int(sum_narrow(v)),
        Column::I16(v) => Sum::Int(sum_narrow(v)),
        Column::I32(v) => Sum::Int(sum_narrow(v)),
        Column::U8(v) => Sum::Int(sum_narrow(v)),
        Column::U16(v) => Sum::Int(sum_narrow(v)),
        Column::U32(v) => Sum::Int(sum_narrow(v)),
        Column::I64(v) => Sum::Int(sum_wide(v, |x| x as u64 ^ (1 << 63), 1 << 63)),
        Column::U64(v) => Sum::Int(sum_wide(v, |x| x, 0)),
        Column::F32(v) => float_sum(v.iter().map(|&x| x as f64)),
        Column::F64(v) => float_sum(v.iter().copied()),
        Column::Str(v) if v.is_empty() => Sum::Int(0),
        Column::Str(_) => return Err(KernelError::Unsupported { op: "sum", dtype: DType::Str }),
    })
}

/// Sum per block with `block_sum`, combining in `i128`. Overflow is impossible: a `Vec` spans
/// at most `isize::MAX` bytes, so even 8-byte elements below `2^64` give `|sum| < 2^124`.
fn sum_blocks<T: Sync>(v: &[T], block_sum: impl Fn(&[T]) -> i128 + Sync) -> i128 {
    if v.len() >= PAR_SCAN_MIN {
        v.par_chunks(SUM_BLOCK).map(&block_sum).sum()
    } else {
        v.chunks(SUM_BLOCK).map(&block_sum).sum()
    }
}

/// Exact sum of ≤ 32-bit integers: an `i64` accumulator cannot overflow within a block.
fn sum_narrow<T: Copy + Into<i64> + Sync>(v: &[T]) -> i128 {
    sum_blocks(v, |block| block.iter().map(|&x| x.into()).sum::<i64>() as i128)
}

/// Exact sum of 64-bit integers. Each element is biased into `u64` (`to_unsigned(x) = x + bias`)
/// and split into 32-bit halves summed in `u64` lanes (no overflow within a block of `2^16`),
/// which vectorizes with plain 64-bit adds; the bias is removed at the end.
fn sum_wide<T: Copy + Sync>(v: &[T], to_unsigned: impl Fn(T) -> u64 + Sync, bias: i128) -> i128 {
    sum_blocks(v, |block| {
        let (mut hi, mut lo) = (0u64, 0u64);
        for &x in block {
            let u = to_unsigned(x);
            hi += u >> 32;
            lo += u & 0xFFFF_FFFF;
        }
        ((hi as i128) << 32) + lo as i128 - bias * block.len() as i128
    })
}

/// Python's float `sum()` (`builtin_sum_impl`, CPython ≥ 3.12): the implicit start `0` meets
/// the first item as `int + float`, then Neumaier compensated summation of the rest.
fn float_sum(mut items: impl Iterator<Item = f64>) -> Sum {
    let Some(first) = items.next() else {
        return Sum::Int(0);
    };
    let mut total = CompensatedSum { hi: 0.0 + first, lo: 0.0 };
    for x in items {
        total.add(x);
    }
    Sum::Float(total.finish())
}

/// CPython's `CompensatedSum` (`cs_add` / `cs_to_double`), operation for operation.
#[derive(Clone, Copy)]
struct CompensatedSum {
    hi: f64,
    lo: f64,
}

impl CompensatedSum {
    #[inline(always)]
    fn add(&mut self, x: f64) {
        let t = self.hi + x;
        // Selecting the operands (instead of branching) keeps the loop free of mispredictions.
        let (big, small) = if self.hi.abs() >= x.abs() { (self.hi, x) } else { (x, self.hi) };
        self.lo += (big - t) + small;
        self.hi = t;
    }

    fn finish(self) -> f64 {
        // Keep the sign of a negative zero and never turn an infinite sum into NaN.
        if self.lo != 0.0 && self.lo.is_finite() { self.hi + self.lo } else { self.hi }
    }
}

// ---------------------------------------------------------------------------------------
// min / max
// ---------------------------------------------------------------------------------------

/// Python `min()` semantics (first minimal element by `<`; `None` when empty).
pub fn min(column: &Column) -> Option<Value> {
    extreme::<false>(column)
}

/// Python `max()` semantics.
pub fn max(column: &Column) -> Option<Value> {
    extreme::<true>(column)
}

/// Position of the element [`min`] returns.
pub fn argmin(column: &Column) -> Option<usize> {
    arg_extreme::<false>(column)
}

/// Position of the element [`max`] returns.
pub fn argmax(column: &Column) -> Option<usize> {
    arg_extreme::<true>(column)
}

fn extreme<const MAX: bool>(column: &Column) -> Option<Value> {
    numeric!(column, v => numeric_extreme::<_, MAX>(v).map(Element::into_value),
        s => sequential_extreme::<_, MAX>(s).map(|i| Value::Str(s[i].clone())))
}

fn arg_extreme<const MAX: bool>(column: &Column) -> Option<usize> {
    numeric!(column, v => {
        let m = numeric_extreme::<_, MAX>(v)?;
        if m.is_nan() { Some(0) } else { position(v, |x| x == m) }
    }, s => sequential_extreme::<_, MAX>(s))
}

/// CPython's `min_max` loop: keep the first item, replace it only when an item is strictly
/// better.
fn sequential_extreme<T: PartialOrd, const MAX: bool>(v: &[T]) -> Option<usize> {
    let mut best = 0;
    for (i, x) in v.iter().enumerate().skip(1) {
        if if MAX { *x > v[best] } else { *x < v[best] } {
            best = i;
        }
    }
    (!v.is_empty()).then_some(best)
}

/// The element CPython's `min_max` loop returns, computed with independent lanes (and blocks
/// in parallel). A leading NaN wins (nothing compares below or above it); later NaNs never
/// replace anything. Equal non-NaN elements are bit-identical except `±0.0`, where Python
/// keeps the first one, so the order of the reduction is otherwise unobservable.
fn numeric_extreme<T: Numeric, const MAX: bool>(v: &[T]) -> Option<T> {
    let first = *v.first()?;
    if first.is_nan() {
        return Some(first);
    }
    let pick = |best: T, x: T| if MAX && x > best || !MAX && x < best { x } else { best };
    let block_extreme = |block: &[T]| {
        const LANES: usize = 16;
        let mut lanes = [first; LANES];
        let (chunks, tail) = block.as_chunks::<LANES>();
        for chunk in chunks {
            for (best, &x) in lanes.iter_mut().zip(chunk) {
                *best = pick(*best, x);
            }
        }
        lanes.into_iter().chain(tail.iter().copied()).fold(first, pick)
    };
    let m = if v.len() >= PAR_SCAN_MIN {
        v.par_chunks(SUM_BLOCK).map(block_extreme).reduce(|| first, pick)
    } else {
        block_extreme(v)
    };
    if m.is_float_zero() { v.iter().copied().find(|x| x.is_float_zero()) } else { Some(m) }
}

// ---------------------------------------------------------------------------------------
// sort / argsort
// ---------------------------------------------------------------------------------------

/// Stable in-place sort (`list.sort(reverse=...)` semantics; NaNs sort last and never panic).
///
/// Ordering is Python's `<`; `reverse=true` keeps equal elements in their original order.
/// NaNs (whose position Python leaves ill-defined) go last in both directions, in their
/// original order.
pub fn sort(column: &mut Column, reverse: bool) {
    match column {
        Column::Bool(v) => sort_bool(v, reverse),
        Column::I8(v) => counting_sort(v, reverse, i8::MIN..=i8::MAX),
        Column::U8(v) => counting_sort(v, reverse, u8::MIN..=u8::MAX),
        Column::I16(v) if v.len() >= COUNTING_SORT_16_MIN => counting_sort(v, reverse, i16::MIN..=i16::MAX),
        Column::U16(v) if v.len() >= COUNTING_SORT_16_MIN => counting_sort(v, reverse, u16::MIN..=u16::MAX),
        Column::I16(v) => sort_total(v, reverse),
        Column::U16(v) => sort_total(v, reverse),
        Column::I32(v) => sort_total(v, reverse),
        Column::U32(v) => sort_total(v, reverse),
        Column::I64(v) => sort_total(v, reverse),
        Column::U64(v) => sort_total(v, reverse),
        Column::F32(v) => sort_floats(v, reverse),
        Column::F64(v) => sort_floats(v, reverse),
        Column::Str(v) => sort_total(v, reverse),
    }
}

/// Stable argsort with the same ordering as [`sort`].
pub fn argsort(column: &Column, reverse: bool) -> Vec<usize> {
    numeric!(column, v => argsort_numeric(v, reverse), s => {
        // `(string, index)` pairs are distinct, so an unstable sort gives the stable order.
        let mut pairs: Vec<(&str, usize)> = s.iter().map(|x| x.as_str()).zip(0..).collect();
        if reverse {
            sort_unstable_by(&mut pairs, |(x, i), (y, j)| y.cmp(x).then(i.cmp(j)));
        } else {
            sort_unstable_by(&mut pairs, <(&str, usize)>::cmp);
        }
        pairs.into_iter().map(|(_, i)| i).collect()
    })
}

fn sort_unstable_by<T: Send>(v: &mut [T], cmp: impl Fn(&T, &T) -> Ordering + Sync) {
    if v.len() >= PAR_SORT_MIN { v.par_sort_unstable_by(cmp) } else { v.sort_unstable_by(cmp) }
}

/// For types whose equal elements are indistinguishable, an unstable sort is observably
/// stable (in either direction).
fn sort_total<T: Ord + Send>(v: &mut [T], reverse: bool) {
    if reverse { sort_unstable_by(v, |a, b| b.cmp(a)) } else { sort_unstable_by(v, T::cmp) }
}

fn sort_bool(v: &mut [bool], reverse: bool) {
    let trues = count_where(v, |b| b);
    let (first, first_len) = if reverse { (true, trues) } else { (false, v.len() - trues) };
    v[..first_len].fill(first);
    v[first_len..].fill(!first);
}

/// Counting sort over a small integer type; `universe` lists every value in ascending order,
/// matching the rank given by [`Numeric::key`].
fn counting_sort<T, U>(v: &mut [T], reverse: bool, universe: U)
where
    T: Numeric,
    U: DoubleEndedIterator<Item = T> + ExactSizeIterator,
{
    let mut counts = vec![0usize; universe.len()];
    for &x in v.iter() {
        counts[x.key() as usize] += 1;
    }
    let runs = universe.zip(counts);
    let mut start = 0;
    let mut fill = |(value, n): (T, usize)| {
        v[start..start + n].fill(value);
        start += n;
    };
    if reverse { runs.rev().for_each(&mut fill) } else { runs.for_each(&mut fill) }
}

/// NaNs are moved to the end (original order). The rest is sorted unstably by `<`, which is
/// observably stable except for `±0.0`: those form one run, which is rewritten with the zeros
/// in their original order.
fn sort_floats<T: Numeric>(v: &mut Vec<T>, reverse: bool) {
    let nans = take_nans(v);
    let zeros = mixed_zeros(v);
    if reverse {
        sort_unstable_by(v, |a, b| b.partial_cmp(a).unwrap_or(Ordering::Equal));
    } else {
        sort_unstable_by(v, |a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
    }
    if let Some(zeros) = zeros {
        let zero = zeros[0];
        let start = v.partition_point(|&x| if reverse { x > zero } else { x < zero });
        v[start..start + zeros.len()].copy_from_slice(&zeros);
    }
    v.extend(nans);
}

/// Remove the NaNs from `v` (keeping the order of the rest) and return them in order.
fn take_nans<T: Numeric>(v: &mut Vec<T>) -> Vec<T> {
    let mut nans = Vec::new();
    if v.iter().any(|x| x.is_nan()) {
        v.retain(|&x| {
            let nan = x.is_nan();
            if nan {
                nans.push(x);
            }
            !nan
        });
    }
    nans
}

/// The zeros of `v` in order, if both `-0.0` and `0.0` occur (otherwise order is invisible).
fn mixed_zeros<T: Numeric>(v: &[T]) -> Option<Vec<T>> {
    let (neg, pos) = v.iter().fold((false, false), |(neg, pos), &x| {
        let zero = x.is_float_zero();
        (neg | (zero && x.is_sign_negative()), pos | (zero && !x.is_sign_negative()))
    });
    (neg && pos).then(|| v.iter().copied().filter(|x| x.is_float_zero()).collect())
}

/// Stable argsort of numbers: sort unique `(key, index)` pairs, then append NaN positions.
fn argsort_numeric<T: Numeric>(v: &[T], reverse: bool) -> Vec<usize> {
    if T::DTYPE == DType::Bool {
        // `false` then `true` (reversed: `true` then `false`), each in index order.
        let first = |i: &usize| (v[*i].key() == 1) == reverse;
        let mut order: Vec<usize> = (0..v.len()).filter(first).collect();
        order.extend((0..v.len()).filter(|i| !first(i)));
        return order;
    }
    if !v.iter().any(|x| x.is_nan()) {
        return argsort_keys(v.iter().map(|x| x.key()).enumerate(), v.len(), T::KEY_BITS, reverse);
    }
    let mut nans = Vec::new();
    let keyed = v.iter().enumerate().filter_map(|(i, &x)| {
        if x.is_nan() {
            nans.push(i);
            None
        } else {
            Some((i, x.key()))
        }
    });
    let mut order = argsort_keys(keyed, v.len(), T::KEY_BITS, reverse);
    order.extend(nans);
    order
}

/// Sort `(index, key)` pairs by key, then index, returning the indices. The pairs are distinct,
/// so an unstable sort yields the stable order; when key and index both fit in 32 bits they are
/// packed into one `u64` (complemented key bits above 32 are shifted out). `reverse`
/// complements the key, reversing the key order while indices stay ascending.
fn argsort_keys(keyed: impl Iterator<Item = (usize, u64)>, n: usize, bits: u32, reverse: bool) -> Vec<usize> {
    let flip = if reverse { u64::MAX } else { 0 };
    if bits <= 32 && n as u64 <= 1 << 32 {
        let mut packed = Vec::with_capacity(n);
        packed.extend(keyed.map(|(i, k)| (k ^ flip) << 32 | i as u64));
        sort_unstable_by(&mut packed, u64::cmp);
        packed.into_iter().map(|p| (p & 0xFFFF_FFFF) as usize).collect()
    } else {
        let mut pairs = Vec::with_capacity(n);
        pairs.extend(keyed.map(|(i, k)| (k ^ flip, i)));
        sort_unstable_by(&mut pairs, <(u64, usize)>::cmp);
        pairs.into_iter().map(|(_, i)| i).collect()
    }
}

// ---------------------------------------------------------------------------------------
// search
// ---------------------------------------------------------------------------------------

/// What a needle matches in a column of one numeric type.
enum Target<T> {
    Never,
    Equal(T),
    Nan,
}

impl<T: Numeric> Target<T> {
    fn of(needle: &Value) -> Self {
        match value_num(needle) {
            None => Target::Never,
            Some(Num::Float(f)) if f.is_nan() => {
                if T::FLOAT {
                    Target::Nan
                } else {
                    Target::Never
                }
            }
            Some(n) => T::from_num(n).map_or(Target::Never, Target::Equal),
        }
    }
}

/// First position in `start..stop` whose element equals `needle`
/// (numeric equality across dtypes is exact, like Python; NaN equals NaN).
///
/// `stop` is clamped to the column length. A Python int needle outside the 64-bit range can
/// still equal a float element: pass it as `Value::F64` when it converts to a float exactly.
pub fn find(column: &Column, needle: &Value, start: usize, stop: usize) -> Option<usize> {
    let stop = stop.min(column.len());
    if start >= stop {
        return None;
    }
    let hit = numeric!(column, v => {
        let v = &v[start..stop];
        match Target::of(needle) {
            Target::Never => None,
            Target::Equal(t) => position(v, |x| x == t),
            Target::Nan => position(v, |x| x.is_nan()),
        }
    }, s => match needle {
        Value::Str(needle) => s[start..stop].iter().position(|x| x == needle),
        _ => None,
    });
    hit.map(|i| start + i)
}

/// `needle in column`.
pub fn contains(column: &Column, needle: &Value) -> bool {
    find(column, needle, 0, column.len()).is_some()
}

/// Number of elements equal to `needle` (same equality as [`find`]).
pub fn count(column: &Column, needle: &Value) -> usize {
    numeric!(column, v => match Target::of(needle) {
        Target::Never => 0,
        Target::Equal(t) => count_where(v, |x| x == t),
        Target::Nan => count_where(v, |x| x.is_nan()),
    }, s => match needle {
        Value::Str(needle) => s.iter().filter(|x| *x == needle).count(),
        _ => 0,
    })
}

// ---------------------------------------------------------------------------------------
// comparison
// ---------------------------------------------------------------------------------------

/// Element-wise equality of two columns of any dtypes (lengths must match for `true`).
pub fn equal(a: &Column, b: &Column) -> bool {
    a.len() == b.len() && first_difference(a, b).is_none()
}

/// Python list ordering: first differing position decides via `op`; otherwise lengths.
///
/// Where Python raises `TypeError` (a non-empty `str` column against a non-empty numeric
/// one) this returns `false`; use [`try_lex_compare`] to detect that case.
pub fn lex_compare(a: &Column, b: &Column, op: CmpOp) -> bool {
    try_lex_compare(a, b, op).unwrap_or(false)
}

/// [`lex_compare`], or `None` where Python raises `TypeError` (`'<' not supported between
/// instances of 'int' and 'str'`): the first differing pair is a `str` and a number. A `str`
/// never equals a number, so that pair is always at index 0.
pub fn try_lex_compare(a: &Column, b: &Column, op: CmpOp) -> Option<bool> {
    match first_difference(a, b) {
        None => Some(op.holds(Some(a.len().cmp(&b.len())))),
        Some(i) => value_cmp(&a.get(i)?, &b.get(i)?, op),
    }
}

/// First index below both lengths where the elements differ (kernel equality).
fn first_difference(a: &Column, b: &Column) -> Option<usize> {
    let floats = a.dtype().is_float() || b.dtype().is_float();
    if a.dtype() == b.dtype() && !floats {
        // Equality is bitwise here (or `str` equality), so whole blocks compare as slices.
        return crate::with_column_pair!(a, b, (x, y) => block_mismatch(x, y), _ => None);
    }
    let neither_empty = || (!a.is_empty() && !b.is_empty()).then_some(0);
    numeric!(a, x => numeric!(b, y => mismatch(x, y), _t => neither_empty()), _s => neither_empty())
}

fn block_mismatch<T: PartialEq>(a: &[T], b: &[T]) -> Option<usize> {
    const BLOCK: usize = 512;
    let n = a.len().min(b.len());
    let (a, b) = (&a[..n], &b[..n]);
    let k = a.chunks(BLOCK).zip(b.chunks(BLOCK)).position(|(p, q)| p != q)?;
    let start = k * BLOCK;
    a[start..].iter().zip(&b[start..]).position(|(x, y)| x != y).map(|i| start + i)
}

/// First difference of two numeric slices of any (possibly different) types.
fn mismatch<A: Numeric, B: Numeric>(a: &[A], b: &[B]) -> Option<usize> {
    const BLOCK: usize = 32;
    let n = a.len().min(b.len());
    let differs = |x: A, y: B| !elem_eq(x, y);
    for (k, (p, q)) in a[..n].chunks(BLOCK).zip(b[..n].chunks(BLOCK)).enumerate() {
        if p.iter().zip(q).fold(false, |d, (&x, &y)| d | differs(x, y)) {
            return p.iter().zip(q).position(|(&x, &y)| differs(x, y)).map(|i| k * BLOCK + i);
        }
    }
    None
}

/// Python `a == b` for two native values (IEEE: NaN != NaN; `str` never equals a number).
pub fn value_eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Str(x), Value::Str(y)) => x == y,
        _ => match (value_num(a), value_num(b)) {
            (Some(x), Some(y)) => num_cmp(x, y) == Some(Ordering::Equal),
            _ => false,
        },
    }
}

/// Python `a <op> b` for two native values (exact across int/float dtypes; NaN satisfies no
/// operator). `None` where Python raises `TypeError` (a `str` against a number).
pub fn value_cmp(a: &Value, b: &Value, op: CmpOp) -> Option<bool> {
    let ordering = match (a, b) {
        (Value::Str(x), Value::Str(y)) => Some(x.cmp(y)),
        _ => num_cmp(value_num(a)?, value_num(b)?),
    };
    Some(op.holds(ordering))
}

#[cfg(test)]
mod tests;
