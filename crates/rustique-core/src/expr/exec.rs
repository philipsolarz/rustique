//! The block evaluator: typed slice views, the generic element loops, per-node kernel
//! dispatch, and the chunked (and, for large columns, parallel) driver.
//!
//! A program runs over blocks of `chunk` elements. For each block every node runs once,
//! reading its operands' block results (or the input slice, or a broadcast constant) and
//! writing its own reusable buffer — the root writes straight into the output column. Type
//! dispatch happens once per node and block; the loops inside are monomorphic.

use std::cmp::Ordering;
use std::ops::Range;
use std::sync::atomic::{AtomicUsize, Ordering as Atomic};

use compact_str::CompactString;
use rayon::prelude::*;

use crate::column::{Column, Value};
use crate::dtype::DType;

use super::kernels::{self, Bounds, Fault, Num, PyFloat, PyInt};
use super::plan::{self, Cmp, Failure, Kind, PNode, Program};
use super::{BinaryOp, ErrorKind, Expr, ExprError, ExprType, IntMode, UnaryOp, text};

/// Block size and parallelism threshold.
#[derive(Clone, Copy, Debug)]
pub(super) struct Tuning {
    pub chunk: usize,
    /// Columns at least this long are split across the rayon pool.
    pub parallel_min: usize,
}

impl Tuning {
    pub const DEFAULT: Tuning = Tuning { chunk: 4096, parallel_min: 1 << 17 };
}

/// Longest string a native `str * n` builds; longer results are left to Python.
const REPEAT_LIMIT: u128 = 1 << 28;

pub(super) fn bug() -> ExprError {
    ExprError::raise(ErrorKind::SystemError, "internal error in the rustique expression engine")
}

/// Typed access to the slice views.
pub(super) trait Elem: Sized + Send + Sync + 'static {
    fn arr(a: Arr<'_>) -> Option<&[Self]>;
    fn arr_mut(a: ArrMut<'_>) -> Option<&mut [Self]>;
}

macro_rules! views {
    ($($v:ident: $t:ty),* $(,)?) => {
        /// A borrowed, typed slice of column data.
        #[derive(Clone, Copy)]
        pub(super) enum Arr<'a> { $($v(&'a [$t])),* }

        /// A mutable, typed slice of column data.
        pub(super) enum ArrMut<'a> { $($v(&'a mut [$t])),* }

        impl<'a> Arr<'a> {
            pub fn of(column: &'a Column) -> Self {
                match column { $(Column::$v(v) => Arr::$v(v)),* }
            }
            pub fn len(self) -> usize {
                match self { $(Arr::$v(s) => s.len()),* }
            }
            pub fn dtype(self) -> DType {
                match self { $(Arr::$v(_) => DType::$v),* }
            }
            pub fn slice(self, range: Range<usize>) -> Self {
                match self { $(Arr::$v(s) => Arr::$v(&s[range])),* }
            }
            pub fn value(self, i: usize) -> Option<Value> {
                match self { $(Arr::$v(s) => s.get(i).cloned().map(Value::$v)),* }
            }
        }

        impl<'a> ArrMut<'a> {
            pub fn of(column: &'a mut Column) -> Self {
                match column { $(Column::$v(v) => ArrMut::$v(v)),* }
            }
            pub fn len(&self) -> usize {
                match self { $(ArrMut::$v(s) => s.len()),* }
            }
            pub fn dtype(&self) -> DType {
                match self { $(ArrMut::$v(_) => DType::$v),* }
            }
            pub fn slice(self, range: Range<usize>) -> Self {
                match self { $(ArrMut::$v(s) => ArrMut::$v(&mut s[range])),* }
            }
            pub fn split_at(self, mid: usize) -> (Self, Self) {
                match self {
                    $(ArrMut::$v(s) => {
                        let (a, b) = s.split_at_mut(mid);
                        (ArrMut::$v(a), ArrMut::$v(b))
                    })*
                }
            }
            /// Copy `src` (same dtype), broadcasting a one-element operand.
            pub fn assign(self, src: Opd<'_>) -> bool {
                match (self, src.arr) {
                    $((ArrMut::$v(d), Arr::$v(s)) => {
                        match (src.one, s.first()) {
                            (true, Some(x)) => d.fill(x.clone()),
                            _ => d.clone_from_slice(&s[..d.len()]),
                        }
                        true
                    })*
                    _ => false,
                }
            }
        }

        $(impl Elem for $t {
            #[inline(always)]
            fn arr(a: Arr<'_>) -> Option<&[Self]> {
                match a { Arr::$v(s) => Some(s), _ => None }
            }
            #[inline(always)]
            fn arr_mut(a: ArrMut<'_>) -> Option<&mut [Self]> {
                match a { ArrMut::$v(s) => Some(s), _ => None }
            }
        })*

        /// A column of `len` zeros / `false` / empty strings (zeroed memory where possible).
        pub(super) fn zeroed(dtype: DType, len: usize) -> Column {
            match dtype { $(DType::$v => Column::$v(vec![<$t>::default(); len])),* }
        }
    };
}

views!(
    Bool: bool, I8: i8, I16: i16, I32: i32, I64: i64, U8: u8, U16: u16, U32: u32, U64: u64,
    F32: f32, F64: f64, Str: CompactString,
);

/// Run `$body` with `$T` bound to the Rust type of an integer dtype.
macro_rules! with_int {
    ($dtype:expr, $T:ident => $body:expr, _ => $other:expr) => {
        match $dtype {
            DType::I8 => { type $T = i8; $body }
            DType::I16 => { type $T = i16; $body }
            DType::I32 => { type $T = i32; $body }
            DType::I64 => { type $T = i64; $body }
            DType::U8 => { type $T = u8; $body }
            DType::U16 => { type $T = u16; $body }
            DType::U32 => { type $T = u32; $body }
            DType::U64 => { type $T = u64; $body }
            _ => $other,
        }
    };
}

/// Run `$body` with `$T` bound to the Rust type of a bool, integer or float dtype.
macro_rules! with_num {
    ($dtype:expr, $T:ident => $body:expr, _ => $other:expr) => {
        match $dtype {
            DType::Bool => { type $T = bool; $body }
            DType::F32 => { type $T = f32; $body }
            DType::F64 => { type $T = f64; $body }
            d => with_int!(d, $T => $body, _ => $other),
        }
    };
}

/// An operand: a block of values, or a one-element constant broadcast over the block.
#[derive(Clone, Copy)]
pub(super) struct Opd<'a> {
    pub arr: Arr<'a>,
    pub one: bool,
}

impl<'a> Opd<'a> {
    fn side<T: Elem>(self) -> Result<Side<'a, T>, Bug> {
        let s = T::arr(self.arr).ok_or(Bug)?;
        Ok(if self.one { Side::One(s.first().ok_or(Bug)?) } else { Side::Col(s) })
    }

    fn value(self, i: usize) -> Option<Value> {
        self.arr.value(if self.one { 0 } else { i })
    }
}

/// A typed operand.
#[derive(Clone, Copy)]
enum Side<'a, T> {
    Col(&'a [T]),
    One(&'a T),
}

impl<'a, T> Side<'a, T> {
    #[inline(always)]
    fn get(&self, i: usize) -> &'a T {
        match *self {
            Side::Col(s) => &s[i],
            Side::One(x) => x,
        }
    }
}

impl<T: Copy> Side<'_, T> {
    #[inline(always)]
    fn at(&self, i: usize) -> T {
        *self.get(i)
    }
}

/// An internal invariant was violated (operand types disagree with the plan).
#[derive(Debug)]
pub(super) struct Bug;

type Run = Result<Option<(usize, Fault)>, Bug>;

/// `out[i] = f(a[i])`; returns the first index whose `f` reported failure. Failing elements
/// still get a (garbage) value so the loop stays branch-free and vectorisable.
#[inline(always)]
fn map1<A: Copy, R>(a: Side<'_, A>, out: &mut [R], f: impl Fn(A) -> (R, bool)) -> Option<usize> {
    let n = out.len();
    let mut bad = false;
    match a {
        Side::Col(x) => {
            for (o, &x) in out.iter_mut().zip(&x[..n]) {
                let (r, e) = f(x);
                *o = r;
                bad |= e;
            }
        }
        Side::One(&x) => {
            for o in out.iter_mut() {
                let (r, e) = f(x);
                *o = r;
                bad |= e;
            }
        }
    }
    if bad { (0..n).find(|&i| f(a.at(i)).1) } else { None }
}

/// `out[i] = f(a[i], b[i])`, like [`map1`].
#[inline(always)]
fn map2<A: Copy, B: Copy, R>(
    a: Side<'_, A>,
    b: Side<'_, B>,
    out: &mut [R],
    f: impl Fn(A, B) -> (R, bool),
) -> Option<usize> {
    let n = out.len();
    let mut bad = false;
    match (a, b) {
        (Side::Col(x), Side::Col(y)) => {
            for ((o, &x), &y) in out.iter_mut().zip(&x[..n]).zip(&y[..n]) {
                let (r, e) = f(x, y);
                *o = r;
                bad |= e;
            }
        }
        (Side::Col(x), Side::One(&y)) => {
            for (o, &x) in out.iter_mut().zip(&x[..n]) {
                let (r, e) = f(x, y);
                *o = r;
                bad |= e;
            }
        }
        (Side::One(&x), Side::Col(y)) => {
            for (o, &y) in out.iter_mut().zip(&y[..n]) {
                let (r, e) = f(x, y);
                *o = r;
                bad |= e;
            }
        }
        (Side::One(&x), Side::One(&y)) => {
            for o in out.iter_mut() {
                let (r, e) = f(x, y);
                *o = r;
                bad |= e;
            }
        }
    }
    if bad { (0..n).find(|&i| f(a.at(i), b.at(i)).1) } else { None }
}

fn at_fault(fault: Fault) -> impl Fn(usize) -> (usize, Fault) {
    move |i| (i, fault)
}

/// Run one node over a block; failures become the Python error for that element.
fn run_node(node: &PNode, opds: &[Opd<'_>], out: ArrMut<'_>) -> Option<(usize, ExprError)> {
    match kernel(node, opds, out) {
        Ok(None) => None,
        Ok(Some((i, fault))) => Some((i, fault_error(fault, node, opds, i))),
        Err(Bug) => Some((0, bug())),
    }
}

fn kernel(node: &PNode, opds: &[Opd<'_>], out: ArrMut<'_>) -> Run {
    let opd = |k: usize| opds.get(k).copied().ok_or(Bug);
    match &node.kind {
        Kind::Input | Kind::Const(_) => Err(Bug),
        Kind::Convert(_) => convert(opd(0)?, out),
        Kind::Cast(_) => cast(node.ty, opd(0)?, out),
        Kind::Unary(op, _) => unary(*op, opd(0)?, out),
        Kind::Arith(op @ (BinaryOp::LShift | BinaryOp::RShift), ..) if matches!(node.ty, ExprType::Fixed(_)) => {
            fixed_shift(*op == BinaryOp::LShift, opd(0)?, opd(1)?, out)
        }
        Kind::Arith(op, ..) => arith(*op, opd(0)?, opd(1)?, out),
        Kind::Compare(cmp, ..) => compare(*cmp, opd(0)?, opd(1)?, out).map(|()| None),
        Kind::Concat(..) => concat(opd(0)?, opd(1)?, out).map(|()| None),
        Kind::Repeat(..) => repeat(opd(0)?, opd(1)?, out),
    }
}

fn convert(a: Opd<'_>, out: ArrMut<'_>) -> Run {
    with_num!(a.arr.dtype(), S => {
        let a = a.side::<S>()?;
        with_num!(out.dtype(), D => {
            let out = D::arr_mut(out).ok_or(Bug)?;
            Ok(map1(a, out, |x| match D::from_num(x) {
                Some(v) => (v, false),
                None => (D::default(), true),
            })
            .map(at_fault(Fault::Overflow)))
        }, _ => Err(Bug))
    }, _ => Err(Bug))
}

fn cast(to: ExprType, a: Opd<'_>, out: ArrMut<'_>) -> Run {
    let src = a.arr.dtype();
    match to {
        _ if src == DType::Str => cast_text(to, a, out),
        ExprType::Bool => with_num!(src, S => {
            map1(a.side::<S>()?, bool::arr_mut(out).ok_or(Bug)?, |x| (x.nonzero(), false));
            Ok(None)
        }, _ => Err(Bug)),
        ExprType::Str => Err(Bug),
        // Exact (range-checked) or rounding numeric conversion.
        to if to.is_float() || !src.is_float() => convert(a, out),
        // `int(x)` of a float truncates.
        to => with_num!(src, S => with_int!(to.dtype(), D => {
            let a = a.side::<S>()?;
            let out = D::arr_mut(out).ok_or(Bug)?;
            let truncate = |x: S| kernels::float_to_int::<D>(x.to_f64());
            Ok(map1(a, out, |x| match truncate(x) {
                Ok(v) => (v, false),
                Err(_) => (0, true),
            })
            .map(|i| (i, truncate(a.at(i)).err().unwrap_or(Fault::Overflow))))
        }, _ => Err(Bug)), _ => Err(Bug)),
    }
}

/// `out[i] = f(text[i])` for fallible text functions; returns the first failure.
fn map_text<R: Default>(
    a: Side<'_, CompactString>,
    out: &mut [R],
    f: impl Fn(&str) -> Result<R, Fault>,
) -> Option<(usize, Fault)> {
    let mut first = None;
    for (i, o) in out.iter_mut().enumerate() {
        *o = f(a.get(i)).unwrap_or_else(|fault| {
            first.get_or_insert((i, fault));
            R::default()
        });
    }
    first
}

fn cast_text(to: ExprType, a: Opd<'_>, out: ArrMut<'_>) -> Run {
    let a = a.side::<CompactString>()?;
    Ok(match to {
        ExprType::Bool => map_text(a, bool::arr_mut(out).ok_or(Bug)?, |s| Ok(!s.is_empty())),
        ExprType::Int => map_text(a, i64::arr_mut(out).ok_or(Bug)?, text::parse_int),
        ExprType::Float => map_text(a, f64::arr_mut(out).ok_or(Bug)?, text::parse_float),
        _ => return Err(Bug),
    })
}

fn unary(op: UnaryOp, a: Opd<'_>, out: ArrMut<'_>) -> Run {
    match a.arr.dtype() {
        DType::Bool if op == UnaryOp::Invert => {
            map1(a.side::<bool>()?, bool::arr_mut(out).ok_or(Bug)?, |x| (!x, false));
            Ok(None)
        }
        DType::F32 => float_unary::<f32>(op, a, out),
        DType::F64 => float_unary::<f64>(op, a, out),
        d => with_int!(d, T => int_unary::<T>(op, a, out), _ => Err(Bug)),
    }
}

fn int_unary<T: PyInt + Elem>(op: UnaryOp, a: Opd<'_>, out: ArrMut<'_>) -> Run {
    let a = a.side::<T>()?;
    let out = T::arr_mut(out).ok_or(Bug)?;
    let fail = match op {
        UnaryOp::Neg => map1(a, out, T::py_neg),
        UnaryOp::Abs => map1(a, out, T::py_abs),
        UnaryOp::Invert => map1(a, out, |x| (x.not(), false)),
        UnaryOp::Pos | UnaryOp::IsNan => return Err(Bug),
    };
    Ok(fail.map(at_fault(Fault::Overflow)))
}

fn float_unary<F: PyFloat + Elem>(op: UnaryOp, a: Opd<'_>, out: ArrMut<'_>) -> Run {
    let a = a.side::<F>()?;
    match op {
        UnaryOp::Neg => map1(a, F::arr_mut(out).ok_or(Bug)?, |x| (-x, false)),
        UnaryOp::Abs => map1(a, F::arr_mut(out).ok_or(Bug)?, |x| (x.abs(), false)),
        UnaryOp::IsNan => map1(a, bool::arr_mut(out).ok_or(Bug)?, |x| (x.is_nan(), false)),
        UnaryOp::Pos | UnaryOp::Invert => return Err(Bug),
    };
    Ok(None)
}

fn arith(op: BinaryOp, a: Opd<'_>, b: Opd<'_>, out: ArrMut<'_>) -> Run {
    match a.arr.dtype() {
        DType::Bool => {
            let (a, b) = (a.side::<bool>()?, b.side::<bool>()?);
            let out = bool::arr_mut(out).ok_or(Bug)?;
            match op {
                BinaryOp::BitAnd => map2(a, b, out, |x, y| (x & y, false)),
                BinaryOp::BitOr => map2(a, b, out, |x, y| (x | y, false)),
                BinaryOp::BitXor => map2(a, b, out, |x, y| (x ^ y, false)),
                _ => return Err(Bug),
            };
            Ok(None)
        }
        DType::F32 => float_arith::<f32>(op, a, b, out),
        DType::F64 => float_arith::<f64>(op, a, b, out),
        d => with_int!(d, T => int_arith::<T>(op, a, b, out), _ => Err(Bug)),
    }
}

fn int_arith<T: PyInt + Elem>(op: BinaryOp, a: Opd<'_>, b: Opd<'_>, out: ArrMut<'_>) -> Run {
    use BinaryOp::*;
    let (a, b) = (a.side::<T>()?, b.side::<T>()?);
    if op == TrueDiv {
        let out = f64::arr_mut(out).ok_or(Bug)?;
        return Ok(map2(a, b, out, kernels::int_truediv::<T>).map(at_fault(Fault::ZeroDivision)));
    }
    let out = T::arr_mut(out).ok_or(Bug)?;
    let overflow = at_fault(Fault::Overflow);
    Ok(match op {
        Add => add(a, b, out).map(overflow),
        Sub => sub(a, b, out).map(overflow),
        Mul => mul(a, b, out).map(overflow),
        FloorDiv => map2(a, b, out, T::py_floordiv)
            .map(|i| (i, if b.at(i) == T::ZERO { Fault::ZeroDivision } else { Fault::Overflow })),
        Mod => map2(a, b, out, T::py_mod).map(at_fault(Fault::ZeroDivision)),
        Pow => map2(a, b, out, T::py_pow)
            .map(|i| (i, if b.at(i).is_negative() { Fault::NegativeExponent } else { Fault::Overflow })),
        LShift => map2(a, b, out, T::py_shl)
            .map(|i| (i, if b.at(i).is_negative() { Fault::NegativeShift } else { Fault::Overflow })),
        RShift => map2(a, b, out, T::py_shr).map(at_fault(Fault::NegativeShift)),
        BitAnd => map2(a, b, out, |x, y| (x.and(y), false)).map(overflow),
        BitOr => map2(a, b, out, |x, y| (x.or(y), false)).map(overflow),
        BitXor => map2(a, b, out, |x, y| (x.xor(y), false)).map(overflow),
        _ => return Err(Bug),
    })
}

/// `a << n` / `a >> n` on a fixed-width type, like the rustique scalars: bits shifted out are
/// dropped (`>>` is arithmetic for signed types); a negative count raises `ValueError` and a
/// count of at least the width `OverflowError`. The count is an `int` (i64) or of `a`'s type.
fn fixed_shift(left: bool, a: Opd<'_>, b: Opd<'_>, out: ArrMut<'_>) -> Run {
    let count = b.arr.dtype();
    with_int!(a.arr.dtype(), T => {
        let a = a.side::<T>()?;
        let out = T::arr_mut(out).ok_or(Bug)?;
        Ok(if count == DType::I64 {
            shift_loop(left, a, b.side::<i64>()?, out)
        } else {
            shift_loop(left, a, b.side::<T>()?, out)
        })
    }, _ => Err(Bug))
}

fn shift_loop<T: PyInt, C: Num>(left: bool, a: Side<'_, T>, b: Side<'_, C>, out: &mut [T]) -> Option<(usize, Fault)> {
    let width = i128::from(T::WIDTH);
    let shift = |x: T, n: C| {
        let n = n.to_i128();
        let ok = (0..width).contains(&n);
        let n = if ok { n as u32 } else { 0 };
        (if left { x.shl_bits(n) } else { x.shr_bits(n) }, !ok)
    };
    map2(a, b, out, shift)
        .map(|i| (i, if b.at(i).to_i128() < 0 { Fault::NegativeShift } else { Fault::ShiftRange }))
}

/// `x + c` with a constant checks precomputed bounds instead of per-element overflow flags,
/// which keeps the loop vectorisable.
fn add<T: PyInt>(a: Side<'_, T>, b: Side<'_, T>, out: &mut [T]) -> Option<usize> {
    match (a, b) {
        (x @ Side::Col(_), Side::One(&c)) | (Side::One(&c), x @ Side::Col(_)) => {
            let bounds = Bounds::add(c);
            map1(x, out, |v| (v.wrapping_add(c), bounds.outside(v)))
        }
        _ => map2(a, b, out, T::overflowing_add),
    }
}

fn sub<T: PyInt>(a: Side<'_, T>, b: Side<'_, T>, out: &mut [T]) -> Option<usize> {
    match (a, b) {
        (x @ Side::Col(_), Side::One(&c)) => {
            let bounds = Bounds::sub(c);
            map1(x, out, |v| (v.wrapping_sub(c), bounds.outside(v)))
        }
        (Side::One(&c), x @ Side::Col(_)) => {
            let bounds = Bounds::rsub(c);
            map1(x, out, |v| (c.wrapping_sub(v), bounds.outside(v)))
        }
        _ => map2(a, b, out, T::overflowing_sub),
    }
}

fn mul<T: PyInt>(a: Side<'_, T>, b: Side<'_, T>, out: &mut [T]) -> Option<usize> {
    match (a, b) {
        (x @ Side::Col(_), Side::One(&c)) | (Side::One(&c), x @ Side::Col(_)) => {
            let bounds = Bounds::mul(c);
            map1(x, out, |v| (v.wrapping_mul(c), bounds.outside(v)))
        }
        _ => map2(a, b, out, T::overflowing_mul),
    }
}

fn float_arith<F: PyFloat + Elem>(op: BinaryOp, a: Opd<'_>, b: Opd<'_>, out: ArrMut<'_>) -> Run {
    use BinaryOp::*;
    let (a, b) = (a.side::<F>()?, b.side::<F>()?);
    let out = F::arr_mut(out).ok_or(Bug)?;
    let zero = at_fault(Fault::ZeroDivision);
    Ok(match op {
        Add => map2(a, b, out, |x, y| (x + y, false)).map(zero),
        Sub => map2(a, b, out, |x, y| (x - y, false)).map(zero),
        Mul => map2(a, b, out, |x, y| (x * y, false)).map(zero),
        TrueDiv => map2(a, b, out, |x, y| (x / y, y == F::ZERO)).map(zero),
        FloorDiv => map2(a, b, out, |x, y| (kernels::float_floordiv(x, y), y == F::ZERO)).map(zero),
        Mod => map2(a, b, out, |x, y| (kernels::float_mod(x, y), y == F::ZERO)).map(zero),
        Pow => map2(a, b, out, |x, y| match kernels::float_pow(x, y) {
            Ok(v) => (v, false),
            Err(_) => (F::ZERO, true),
        })
        .map(|i| (i, kernels::float_pow(a.at(i), b.at(i)).err().unwrap_or(Fault::Overflow))),
        _ => return Err(Bug),
    })
}

fn compare(cmp: Cmp, a: Opd<'_>, b: Opd<'_>, out: ArrMut<'_>) -> Result<(), Bug> {
    let out = bool::arr_mut(out).ok_or(Bug)?;
    match (a.arr.dtype(), b.arr.dtype()) {
        (DType::Str, DType::Str) => {
            let (a, b) = (a.side::<CompactString>()?, b.side::<CompactString>()?);
            for (i, o) in out.iter_mut().enumerate() {
                *o = cmp.test(Some(a.get(i).as_str().cmp(b.get(i).as_str())));
            }
        }
        (x, y) if x == y => with_num!(x, T => same_compare::<T>(cmp, a.side()?, b.side()?, out), _ => return Err(Bug)),
        (DType::I64, DType::F64) => {
            map2(a.side::<i64>()?, b.side::<f64>()?, out, |x, y| (cmp.test(kernels::int_float_cmp(x, y)), false));
        }
        (DType::U64, DType::F64) => {
            map2(a.side::<u64>()?, b.side::<f64>()?, out, |x, y| (cmp.test(kernels::int_float_cmp(x, y)), false));
        }
        (DType::U64, DType::I64) => {
            map2(a.side::<u64>()?, b.side::<i64>()?, out, |x, y| {
                let ord = if y < 0 { Ordering::Greater } else { x.cmp(&(y as u64)) };
                (cmp.test(Some(ord)), false)
            });
        }
        _ => return Err(Bug),
    }
    Ok(())
}

fn same_compare<T: Num>(cmp: Cmp, a: Side<'_, T>, b: Side<'_, T>, out: &mut [bool]) {
    match cmp {
        Cmp::Eq => map2(a, b, out, |x, y| (x == y, false)),
        Cmp::Ne => map2(a, b, out, |x, y| (x != y, false)),
        Cmp::Lt => map2(a, b, out, |x, y| (x < y, false)),
        Cmp::Le => map2(a, b, out, |x, y| (x <= y, false)),
        Cmp::Gt => map2(a, b, out, |x, y| (x > y, false)),
        Cmp::Ge => map2(a, b, out, |x, y| (x >= y, false)),
    };
}

fn concat(a: Opd<'_>, b: Opd<'_>, out: ArrMut<'_>) -> Result<(), Bug> {
    let (a, b) = (a.side::<CompactString>()?, b.side::<CompactString>()?);
    for (i, o) in CompactString::arr_mut(out).ok_or(Bug)?.iter_mut().enumerate() {
        let (x, y) = (a.get(i), b.get(i));
        let mut s = CompactString::with_capacity(x.len() + y.len());
        s.push_str(x);
        s.push_str(y);
        *o = s;
    }
    Ok(())
}

fn repeat(text: Opd<'_>, count: Opd<'_>, out: ArrMut<'_>) -> Run {
    let (text, count) = (text.side::<CompactString>()?, count.side::<i64>()?);
    let mut first = None;
    for (i, o) in CompactString::arr_mut(out).ok_or(Bug)?.iter_mut().enumerate() {
        let (s, n) = (text.get(i), count.at(i));
        *o = if n <= 0 || s.is_empty() {
            CompactString::default()
        } else if s.len() as u128 * n as u128 > REPEAT_LIMIT {
            first.get_or_insert((i, Fault::Fallback));
            CompactString::default()
        } else {
            s.repeat(n as usize)
        };
    }
    Ok(first)
}

/// The Python error for a failed element `i` of `node`.
fn fault_error(fault: Fault, node: &PNode, opds: &[Opd<'_>], i: usize) -> ExprError {
    use ErrorKind::*;
    let fixed = matches!(node.ty, ExprType::Fixed(_));
    let operand = |k: usize| opds.get(k).and_then(|o| o.value(i));
    let shown = |k: usize| operand(k).map_or_else(String::new, |v| text::format_value(&v));
    match fault {
        Fault::Overflow if fixed => {
            let what = match &node.kind {
                Kind::Arith(op, ..) => format!("{} {} {}", shown(0), op.symbol(), shown(1)),
                Kind::Unary(UnaryOp::Neg, _) => format!("-({})", shown(0)),
                Kind::Unary(UnaryOp::Abs, _) => format!("abs({})", shown(0)),
                _ => shown(0),
            };
            ExprError::raise(OverflowError, format!("{what} is out of range for {}", node.ty))
        }
        Fault::Overflow | Fault::Fallback => ExprError::Overflow,
        Fault::ZeroDivision => ExprError::raise(ZeroDivisionError, "division by zero"),
        Fault::ZeroToNegative => ExprError::raise(ZeroDivisionError, "zero to a negative power"),
        Fault::NegativeShift => ExprError::raise(ValueError, "negative shift count"),
        Fault::ShiftRange => ExprError::raise(
            OverflowError,
            format!("shift count out of range for {} (must be < {})", node.ty, node.ty.dtype().bits().unwrap_or(64)),
        ),
        Fault::NegativeExponent if fixed => {
            ExprError::raise(ValueError, "Integers to negative integer powers are not allowed.")
        }
        Fault::Complex if fixed => {
            ExprError::raise(ValueError, "negative number cannot be raised to a fractional power")
        }
        Fault::NegativeExponent | Fault::Complex => ExprError::Overflow,
        Fault::Range => ExprError::raise(RangeError, "Numerical result out of range"),
        Fault::NanToInt => ExprError::raise(ValueError, "cannot convert float NaN to integer"),
        Fault::InfToInt => ExprError::raise(OverflowError, "cannot convert float infinity to integer"),
        Fault::InvalidInt | Fault::InvalidFloat => {
            let Some(Value::Str(s)) = operand(0) else { return bug() };
            let message = if fault == Fault::InvalidInt {
                text::invalid_int_message(&s)
            } else {
                text::invalid_float_message(&s)
            };
            ExprError::raise(ValueError, message)
        }
    }
}

/// Evaluate a node whose operands are all constants (planning-time folding).
pub(super) fn fold(nodes: &[PNode], index: usize) -> Result<Column, ExprError> {
    let node = &nodes[index];
    let (args, count) = node.kind.operands();
    let mut opds = Vec::with_capacity(count);
    for &j in &args[..count] {
        match &nodes[j].kind {
            Kind::Const(c) => opds.push(Opd { arr: Arr::of(c), one: false }),
            _ => return Err(bug()),
        }
    }
    let mut out = zeroed(node.ty.dtype(), 1);
    match run_node(node, &opds, ArrMut::of(&mut out)) {
        None => Ok(out),
        Some((_, error)) => Err(error),
    }
}

fn record(best: &mut Option<Failure>, failure: Failure) {
    if best.as_ref().is_none_or(|b| failure.precedes(b)) {
        *best = Some(failure);
    }
}

fn earliest(a: Option<Failure>, b: Option<Failure>) -> Option<Failure> {
    match (a, b) {
        (Some(a), Some(b)) => Some(if b.precedes(&a) { b } else { a }),
        (a, b) => a.or(b),
    }
}

/// The operand view of node `j` for the current block.
fn operand<'a>(program: &'a Program, j: usize, input: Arr<'a>, done: &'a [Option<Column>], len: usize) -> Option<Opd<'a>> {
    Some(match &program.nodes.get(j)?.kind {
        Kind::Input => Opd { arr: input, one: false },
        Kind::Const(c) => Opd { arr: Arr::of(c), one: true },
        _ => Opd { arr: Arr::of(done.get(j)?.as_ref()?).slice(0..len), one: false },
    })
}

/// Run every node of `program` over one block (`input` holds elements `base..`). The root
/// writes into `out`; with `out == None` (the prefix run of a stopped plan) nothing does.
fn exec_block(
    program: &Program,
    input: Arr<'_>,
    base: usize,
    mut out: Option<ArrMut<'_>>,
    buffers: &mut [Option<Column>],
    best: &mut Option<Failure>,
) {
    let len = input.len();
    for (i, node) in program.nodes.iter().enumerate() {
        let is_root = program.root == Some(i);
        let (done, rest) = buffers.split_at_mut(i);
        let fail = |best: &mut Option<Failure>, index: usize, error: ExprError| {
            record(best, Failure { index: base + index, order: node.order, error })
        };
        if let Kind::Input | Kind::Const(_) = node.kind {
            if is_root && let Some(target) = out.take() {
                let copied = operand(program, i, input, done, len).is_some_and(|src| target.assign(src));
                if !copied {
                    fail(best, 0, bug());
                }
            }
            continue;
        }
        let (args, count) = node.kind.operands();
        let mut opds = [Opd { arr: input, one: false }; 2];
        for (slot, &j) in opds.iter_mut().zip(&args[..count]) {
            match operand(program, j, input, done, len) {
                Some(o) => *slot = o,
                None => return fail(best, 0, bug()),
            }
        }
        let target = match (is_root, out.take()) {
            (true, Some(target)) => target,
            (_, unused) => {
                out = unused;
                let dtype = node.ty.dtype();
                let buffer = &mut rest[0];
                if buffer.as_ref().is_none_or(|b| b.len() < len || b.dtype() != dtype) {
                    *buffer = Some(zeroed(dtype, len));
                }
                match buffer {
                    Some(b) => ArrMut::of(b).slice(0..len),
                    None => return fail(best, 0, bug()),
                }
            }
        };
        if let Some((index, error)) = run_node(node, &opds[..count], target) {
            fail(best, index, error);
        }
    }
}

/// Evaluate `input` (elements `base..`) block by block into `out`, stopping after the first
/// block with a failure, or before blocks past a failure another worker found (`limit`).
fn run_range(program: &Program, input: Arr<'_>, base: usize, out: ArrMut<'_>, chunk: usize, limit: &AtomicUsize) -> Option<Failure> {
    let mut buffers = vec![None; program.nodes.len()];
    let mut best = None;
    let mut out = out;
    let mut lo = 0;
    while lo < input.len() {
        if base + lo > limit.load(Atomic::Relaxed) {
            break;
        }
        let hi = (lo + chunk).min(input.len());
        let (head, tail) = out.split_at(hi - lo);
        out = tail;
        exec_block(program, input.slice(lo..hi), base + lo, Some(head), &mut buffers, &mut best);
        if let Some(failure) = &best {
            limit.fetch_min(failure.index, Atomic::Relaxed);
            break;
        }
        lo = hi;
    }
    best
}

fn run(program: &Program, input: Arr<'_>, out: ArrMut<'_>, tuning: &Tuning) -> Option<Failure> {
    let n = input.len();
    let chunk = tuning.chunk.max(1);
    let limit = AtomicUsize::new(usize::MAX);
    let threads = rayon::current_num_threads();
    if n < tuning.parallel_min || threads <= 1 {
        return run_range(program, input, 0, out, chunk, &limit);
    }
    let parts = (threads * 4).min(n.div_ceil(chunk));
    let part_len = n.div_ceil(parts).next_multiple_of(chunk);
    let mut pieces = Vec::with_capacity(parts);
    let mut rest = out;
    let mut start = 0;
    while start < n {
        let len = part_len.min(n - start);
        let (head, tail) = rest.split_at(len);
        pieces.push((start, head));
        rest = tail;
        start += len;
    }
    pieces
        .into_par_iter()
        .map(|(start, out)| {
            let len = out.len();
            run_range(program, input.slice(start..start + len), start, out, chunk, &limit)
        })
        .reduce(|| None, earliest)
}

pub(super) fn evaluate(expr: &Expr, column: &Column, mode: IntMode, tuning: &Tuning) -> Result<Column, ExprError> {
    if let Some(error) = expr.limit_error() {
        return Err(error);
    }
    let ty = ExprType::of_column(column.dtype(), mode);
    let program = plan::plan(expr, column.dtype(), ty);
    let input = Arr::of(column);
    if let Some(stop) = program.stop.clone() {
        if column.is_empty() {
            return Err(stop.error);
        }
        // Every element fails at `stop`; operations evaluated before it on the first
        // element may fail first.
        let mut buffers = vec![None; program.nodes.len()];
        let mut best = Some(stop);
        exec_block(&program, input.slice(0..1), 0, None, &mut buffers, &mut best);
        return Err(best.map_or_else(bug, |f| f.error));
    }
    let root = program.root.ok_or_else(bug)?;
    let mut out = zeroed(program.nodes[root].ty.dtype(), column.len());
    match run(&program, input, ArrMut::of(&mut out), tuning) {
        Some(failure) => Err(failure.error),
        None => Ok(out),
    }
}
