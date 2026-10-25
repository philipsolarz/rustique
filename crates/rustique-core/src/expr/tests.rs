use std::cmp::Ordering;
use std::sync::Arc;

use compact_str::CompactString;
use proptest::prelude::*;

use super::exec::{self, Tuning};
use super::kernels::{self, Fault};
use super::*;

const X: fn() -> Arc<Expr> = Expr::element;

fn c(v: impl Into<Value>) -> Arc<Expr> {
    Expr::constant(v)
}

fn bin(op: BinaryOp, a: Arc<Expr>, b: Arc<Expr>) -> Arc<Expr> {
    Expr::binary(op, a, b)
}

fn raise(kind: ErrorKind, message: &str) -> ExprError {
    ExprError::raise(kind, message)
}

fn kind_of(e: &ExprError) -> Option<ErrorKind> {
    match e {
        ExprError::Raise { kind, .. } => Some(*kind),
        ExprError::Overflow => None,
    }
}

use BinaryOp::*;

#[test]
fn arithmetic_and_masks() {
    let col = Column::I64((0..10).collect());
    let e = bin(Add, bin(Mul, X(), c(2i64)), c(1i64));
    assert_eq!(e.to_string(), "((X * 2) + 1)");
    assert_eq!(eval(&e, &col, IntMode::Python), Ok(Column::I64((0..10).map(|x| x * 2 + 1).collect())));
    let m = bin(Eq, bin(Mod, X(), c(3i64)), c(0i64));
    assert_eq!(filter(&m, &col, IntMode::Python), Ok(Column::I64(vec![0, 3, 6, 9])));
    // Truthiness, not just bool results: `X % 3` keeps non-multiples.
    assert_eq!(filter(&bin(Mod, X(), c(3i64)), &col, IntMode::Python).unwrap().len(), 6);
}

#[test]
fn python_division_semantics() {
    let col = Column::I64(vec![-7, 7, -8]);
    let ev = |op, k: i64| eval(&bin(op, X(), c(k)), &col, IntMode::Python);
    assert_eq!(ev(FloorDiv, 2), Ok(Column::I64(vec![-4, 3, -4])));
    assert_eq!(ev(FloorDiv, -2), Ok(Column::I64(vec![3, -4, 4])));
    assert_eq!(ev(Mod, 3), Ok(Column::I64(vec![2, 1, 1])));
    assert_eq!(ev(Mod, -3), Ok(Column::I64(vec![-1, -2, -2])));
    assert_eq!(ev(TrueDiv, 2), Ok(Column::F64(vec![-3.5, 3.5, -4.0])));
    assert_eq!(ev(FloorDiv, 0), Err(raise(ErrorKind::ZeroDivisionError, "division by zero")));
    assert_eq!(ev(Pow, -1), Ok(Column::F64(vec![-1.0 / 7.0, 1.0 / 7.0, -0.125])));
    let floats = Column::F64(vec![7.5, -7.5]);
    assert_eq!(eval(&bin(Mod, X(), c(-2.0)), &floats, IntMode::Python), Ok(Column::F64(vec![-0.5, -1.5])));
    assert_eq!(
        eval(&bin(Mod, X(), c(0.0)), &floats, IntMode::Python),
        Err(raise(ErrorKind::ZeroDivisionError, "division by zero"))
    );
    assert_eq!(
        eval(&bin(Pow, c(0i64), bin(Sub, X(), c(10i64))), &col, IntMode::Python),
        Err(ExprError::Overflow),
        "int ** negative column values are floats: Python must evaluate"
    );
}

#[test]
fn python_int_overflow_falls_back() {
    let col = Column::I64(vec![1, i64::MAX]);
    assert_eq!(eval(&bin(Add, X(), c(1i64)), &col, IntMode::Python), Err(ExprError::Overflow));
    assert_eq!(
        eval(&Expr::unary(UnaryOp::Neg, X()), &Column::I64(vec![i64::MIN]), IntMode::Python),
        Err(ExprError::Overflow)
    );
    assert_eq!(eval(&bin(Mul, X(), X()), &col, IntMode::Python), Err(ExprError::Overflow));
    // Constants beyond i64 compare natively but leave arithmetic to Python.
    let big = c(1u64 << 63);
    assert_eq!(eval(&bin(Lt, X(), big.clone()), &col, IntMode::Python), Ok(Column::Bool(vec![true, true])));
    assert_eq!(eval(&bin(Sub, X(), big.clone()), &col, IntMode::Python), Err(ExprError::Overflow));
    assert_eq!(
        eval(&bin(Add, X(), big.clone()), &Column::F64(vec![0.5]), IntMode::Python),
        Ok(Column::F64(vec![9.223372036854776e18]))
    );
}

#[test]
fn first_failing_element_wins() {
    // Python evaluates element 0 completely first: 1 // 3 is fine, 1 << -2 raises.
    let e = bin(Add, bin(FloorDiv, c(1i64), X()), bin(LShift, c(1i64), bin(Sub, X(), c(5i64))));
    let col = Column::I64(vec![3, 0]);
    assert_eq!(eval(&e, &col, IntMode::Python), Err(raise(ErrorKind::ValueError, "negative shift count")));
    let col = Column::I64(vec![0, 3]);
    assert_eq!(eval(&e, &col, IntMode::Python), Err(raise(ErrorKind::ZeroDivisionError, "division by zero")));
    // A type error raises at the first element, after operations evaluated before it.
    let e = bin(Add, bin(FloorDiv, c(1i64), X()), c("a"));
    assert_eq!(
        kind_of(&eval(&e, &Column::I64(vec![0, 1]), IntMode::Python).unwrap_err()),
        Some(ErrorKind::ZeroDivisionError)
    );
    assert_eq!(
        eval(&e, &Column::I64(vec![1, 0]), IntMode::Python),
        Err(raise(ErrorKind::TypeError, "unsupported operand type(s) for +: 'int' and 'str'"))
    );
    // Static type errors are reported even without elements.
    assert_eq!(kind_of(&eval(&e, &Column::I64(vec![]), IntMode::Python).unwrap_err()), Some(ErrorKind::TypeError));
    // An overflow before a real error means Python sees a big int there: fall back.
    let e = bin(FloorDiv, bin(Mul, X(), c(i64::MAX)), bin(Sub, X(), c(2i64)));
    assert_eq!(eval(&e, &Column::I64(vec![2]), IntMode::Python), Err(ExprError::Overflow));
    assert_eq!(kind_of(&eval(&e, &Column::I64(vec![0, 2]), IntMode::Python).unwrap_err()), None);
    assert_eq!(kind_of(&eval(&e, &Column::I64(vec![1, 2]), IntMode::Python).unwrap_err()), None);
}

#[test]
fn fixed_width_semantics() {
    let col = Column::I8(vec![100, -100]);
    let ev = |e: &Arc<Expr>| eval(e, &col, IntMode::Fixed);
    assert_eq!(ev(&bin(Add, X(), c(27i64))), Ok(Column::I8(vec![127, -73])));
    assert_eq!(ev(&bin(Add, X(), c(28i64))), Err(raise(ErrorKind::OverflowError, "100 + 28 is out of range for i8")));
    assert_eq!(ev(&bin(Add, X(), c(300i64))), Err(raise(ErrorKind::OverflowError, "300 is out of range for i8")));
    assert_eq!(ev(&bin(TrueDiv, X(), c(8i64))), Ok(Column::F64(vec![12.5, -12.5])));
    assert_eq!(ev(&bin(FloorDiv, X(), c(-3i64))), Ok(Column::I8(vec![-34, 33])));
    assert_eq!(kind_of(&ev(&bin(Mul, X(), c(1.5))).unwrap_err()), Some(ErrorKind::TypeError));
    assert_eq!(ev(&bin(Gt, X(), c(1000i64))), Ok(Column::Bool(vec![false, false])));
    assert_eq!(ev(&bin(Gt, X(), c(99.5))), Ok(Column::Bool(vec![true, false])));
    assert_eq!(ev(&bin(Add, bin(Gt, X(), c(0i64)), c(1i64))), Ok(Column::I64(vec![2, 1])));
    assert_eq!(
        ev(&bin(Pow, X(), c(-1i64))),
        Err(raise(ErrorKind::ValueError, "Integers to negative integer powers are not allowed."))
    );
    // Exact Python ints through a cast: overflow falls back instead of raising.
    let wide = bin(Mul, Expr::cast(ExprType::Int, X()), c(i64::MAX));
    assert_eq!(ev(&wide), Err(ExprError::Overflow));
    let u = Column::U8(vec![0, 5]);
    assert_eq!(
        eval(&bin(Sub, X(), c(1i64)), &u, IntMode::Fixed).map_err(|e| kind_of(&e)),
        Err(Some(ErrorKind::OverflowError))
    );
    assert_eq!(eval(&Expr::unary(UnaryOp::Invert, X()), &u, IntMode::Fixed), Ok(Column::U8(vec![255, 250])));
    assert_eq!(
        eval(&bin(Sub, X(), c(-1i64)), &u, IntMode::Fixed).map_err(|e| kind_of(&e)),
        Err(Some(ErrorKind::OverflowError))
    );
    // Different fixed types do not mix; == is False, ordering raises.
    let i16x = Expr::cast(ExprType::Fixed(DType::I16), X());
    assert_eq!(ev(&bin(Eq, i16x.clone(), X())), Ok(Column::Bool(vec![false, false])));
    assert_eq!(kind_of(&ev(&bin(Add, i16x, X())).unwrap_err()), Some(ErrorKind::TypeError));
    let f = Column::F32(vec![1.5, -0.1]);
    assert_eq!(eval(&bin(Mul, X(), c(2i64)), &f, IntMode::Fixed), Ok(Column::F32(vec![3.0, -0.2])));
    assert_eq!(eval(&bin(Lt, X(), c(-0.1)), &f, IntMode::Fixed), Ok(Column::Bool(vec![false, true])));
}

#[test]
fn fixed_width_shifts_drop_bits_and_check_the_count() {
    let i8s = Column::I8(vec![127, -128, 1, -1]);
    let ev = |e: &Arc<Expr>, col: &Column| eval(e, col, IntMode::Fixed);
    assert_eq!(ev(&bin(LShift, X(), c(1i64)), &i8s), Ok(Column::I8(vec![-2, 0, 2, -2])));
    assert_eq!(ev(&bin(RShift, X(), c(7i64)), &i8s), Ok(Column::I8(vec![0, -1, 0, -1])));
    assert_eq!(ev(&bin(LShift, X(), c(0i64)), &i8s), Ok(i8s.clone()));
    let range = raise(ErrorKind::OverflowError, "shift count out of range for i8 (must be < 8)");
    assert_eq!(ev(&bin(LShift, X(), c(8i64)), &i8s), Err(range.clone()));
    assert_eq!(ev(&bin(RShift, X(), c(100i64)), &i8s), Err(range));
    assert_eq!(ev(&bin(RShift, X(), c(-1i64)), &i8s), Err(raise(ErrorKind::ValueError, "negative shift count")));
    let u8s = Column::U8(vec![255, 1, 9]);
    assert_eq!(ev(&bin(LShift, X(), c(1i64)), &u8s), Ok(Column::U8(vec![254, 2, 18])));
    assert_eq!(ev(&bin(LShift, X(), c(-2i64)), &u8s), Err(raise(ErrorKind::ValueError, "negative shift count")));
    // A count of the element type, and a reflected shift (the value must fit the type).
    assert_eq!(
        ev(&bin(LShift, c(1i64), X()), &u8s),
        Err(raise(ErrorKind::OverflowError, "shift count out of range for u8 (must be < 8)"))
    );
    assert_eq!(ev(&bin(LShift, c(1i64), X()), &Column::U8(vec![0, 7])), Ok(Column::U8(vec![1, 128])));
    assert_eq!(kind_of(&ev(&bin(LShift, c(300i64), X()), &u8s).unwrap_err()), Some(ErrorKind::OverflowError));
    let u64s = Column::U64(vec![u64::MAX]);
    assert_eq!(ev(&bin(LShift, X(), c(63i64)), &u64s), Ok(Column::U64(vec![1 << 63])));
    assert_eq!(ev(&bin(RShift, X(), c(64i64)), &u64s).map_err(|e| kind_of(&e)), Err(Some(ErrorKind::OverflowError)));
    // Python ints keep exact semantics: no width, big results fall back to Python.
    let ints = Column::I64(vec![1, -5]);
    assert_eq!(eval(&bin(RShift, X(), c(100i64)), &ints, IntMode::Python), Ok(Column::I64(vec![0, -1])));
    assert_eq!(eval(&bin(LShift, X(), c(64i64)), &ints, IntMode::Python), Err(ExprError::Overflow));
}

#[test]
fn shared_subexpressions_are_planned_once_and_size_is_bounded() {
    let mut e = X();
    for _ in 0..15 {
        e = bin(Add, e.clone(), e);
    }
    assert_eq!(e.size(), (1 << 16) - 1);
    let col = Column::I64(vec![1, 2, 3]);
    assert_eq!(eval(&e, &col, IntMode::Python), Ok(Column::I64(vec![1 << 15, 2 << 15, 3 << 15])));
    assert!(plan::plan(&e, DType::I64, ExprType::Int).nodes.len() < 40);
    let big = bin(Add, e.clone(), e);
    assert_eq!(big.size(), (1 << 17) - 1);
    assert_eq!(kind_of(&eval(&big, &col, IntMode::Python).unwrap_err()), Some(ErrorKind::ValueError));
    assert_eq!(kind_of(&big.result_type(ExprType::Int).unwrap_err()), Some(ErrorKind::ValueError));
    let mut huge = X();
    for _ in 0..200 {
        huge = bin(Mul, huge.clone(), huge);
    }
    assert_eq!(huge.size(), u64::MAX);
    // A failure inside a shared subexpression keeps the rank of its first use: `a + (b + a)`
    // fails in `a` (not in `b`) on an element where both fail.
    let a = bin(FloorDiv, c(1i64), X());
    let b = bin(LShift, c(1i64), bin(Sub, X(), c(5i64)));
    let e = bin(Add, a.clone(), bin(Add, b, a));
    let err = eval(&e, &Column::I64(vec![7, 0]), IntMode::Python).unwrap_err();
    assert_eq!(err, raise(ErrorKind::ZeroDivisionError, "division by zero"));
}

#[test]
fn bools_and_logic() {
    let col = Column::Bool(vec![true, false]);
    let ev = |e: &Arc<Expr>| eval(e, &col, IntMode::Python);
    assert_eq!(ev(&bin(Add, X(), X())), Ok(Column::I64(vec![2, 0])));
    assert_eq!(ev(&Expr::unary(UnaryOp::Invert, X())), Ok(Column::Bool(vec![false, true])));
    assert_eq!(ev(&Expr::unary(UnaryOp::Neg, X())), Ok(Column::I64(vec![-1, 0])));
    assert_eq!(ev(&bin(BitAnd, X(), c(true))), Ok(Column::Bool(vec![true, false])));
    assert_eq!(ev(&bin(BitOr, X(), c(2i64))), Ok(Column::I64(vec![3, 2])));
    assert_eq!(ev(&bin(Eq, X(), c(1i64))), Ok(Column::Bool(vec![true, false])));
    assert_eq!(ev(&bin(Lt, X(), c(0.5))), Ok(Column::Bool(vec![false, true])));
    let ints = Column::I64(vec![1, 5, 9]);
    let mask = bin(BitAnd, bin(Gt, X(), c(2i64)), Expr::unary(UnaryOp::Invert, bin(Gt, X(), c(8i64))));
    assert_eq!(filter(&mask, &ints, IntMode::Python), Ok(Column::I64(vec![5])));
}

#[test]
fn exact_comparisons() {
    let col = Column::I64(vec![(1 << 53) + 1, i64::MAX, -3]);
    assert_eq!(
        eval(&bin(Gt, X(), c(9007199254740992.0)), &col, IntMode::Python),
        Ok(Column::Bool(vec![true, true, false]))
    );
    assert_eq!(
        eval(&bin(Eq, X(), c(9.223_372_036_854_776e18)), &col, IntMode::Python),
        Ok(Column::Bool(vec![false; 3]))
    );
    assert_eq!(eval(&bin(Ne, X(), c(f64::NAN)), &col, IntMode::Python), Ok(Column::Bool(vec![true; 3])));
    let u = Column::U64(vec![u64::MAX, 0]);
    assert_eq!(eval(&bin(Gt, X(), c(-1i64)), &u, IntMode::Fixed), Ok(Column::Bool(vec![true, true])));
    assert_eq!(
        eval(&bin(Lt, X(), bin(Sub, Expr::cast(ExprType::Int, X()), c(1i64))), &Column::U64(vec![5]), IntMode::Fixed),
        Ok(Column::Bool(vec![false]))
    );
}

#[test]
fn strings() {
    let col = Column::from(vec!["ab", "", "b"]);
    let ev = |e: &Arc<Expr>| eval(e, &col, IntMode::Python);
    assert_eq!(ev(&bin(Add, X(), c("!"))), Ok(Column::from(vec!["ab!", "!", "b!"])));
    assert_eq!(ev(&bin(Add, c(">"), X())), Ok(Column::from(vec![">ab", ">", ">b"])));
    assert_eq!(ev(&bin(Mul, c(2i64), X())), Ok(Column::from(vec!["abab", "", "bb"])));
    assert_eq!(ev(&bin(Mul, X(), c(-1i64))), Ok(Column::from(vec!["", "", ""])));
    assert_eq!(ev(&bin(Lt, X(), c("b"))), Ok(Column::Bool(vec![true, true, false])));
    assert_eq!(ev(&bin(Eq, X(), c(1i64))), Ok(Column::Bool(vec![false; 3])));
    assert_eq!(
        ev(&bin(Lt, X(), c(1i64))),
        Err(raise(ErrorKind::TypeError, "'<' not supported between instances of 'str' and 'int'"))
    );
    assert_eq!(
        ev(&bin(Add, X(), c(1i64))),
        Err(raise(ErrorKind::TypeError, "can only concatenate str (not \"int\") to str"))
    );
    assert_eq!(
        ev(&bin(Mul, X(), c(1.5))),
        Err(raise(ErrorKind::TypeError, "can't multiply sequence by non-int of type 'float'"))
    );
    assert_eq!(ev(&bin(Mod, X(), c(1i64))), Err(ExprError::Overflow));
    assert_eq!(ev(&Expr::cast(ExprType::Bool, X())), Ok(Column::Bool(vec![true, false, true])));
    let nums = Column::from(vec![" 12 ", "-1_0"]);
    assert_eq!(eval(&Expr::cast(ExprType::Int, X()), &nums, IntMode::Python), Ok(Column::I64(vec![12, -10])));
    assert_eq!(eval(&Expr::cast(ExprType::Float, X()), &nums, IntMode::Python), Ok(Column::F64(vec![12.0, -10.0])));
    assert_eq!(
        ev(&Expr::cast(ExprType::Int, X())),
        Err(raise(ErrorKind::ValueError, "invalid literal for int() with base 10: 'ab'"))
    );
    assert_eq!(
        eval(&Expr::cast(ExprType::Int, X()), &Column::from(vec!["١"]), IntMode::Python),
        Err(ExprError::Overflow)
    );
    assert_eq!(kind_of(&ev(&Expr::cast(ExprType::Fixed(DType::I8), X())).unwrap_err()), Some(ErrorKind::TypeError));
}

#[test]
fn casts() {
    let f = Column::F64(vec![2.9, -2.9, 1e19]);
    assert_eq!(
        eval(&Expr::cast(ExprType::Int, X()), &Column::F64(vec![2.9, -2.9]), IntMode::Python),
        Ok(Column::I64(vec![2, -2]))
    );
    assert_eq!(eval(&Expr::cast(ExprType::Int, X()), &f, IntMode::Python), Err(ExprError::Overflow));
    assert_eq!(
        eval(&Expr::cast(ExprType::Fixed(DType::U64), X()), &f, IntMode::Python).map_err(|e| kind_of(&e)),
        Err(Some(ErrorKind::OverflowError))
    );
    assert_eq!(
        eval(&Expr::cast(ExprType::Int, X()), &Column::F64(vec![f64::NAN]), IntMode::Python),
        Err(raise(ErrorKind::ValueError, "cannot convert float NaN to integer"))
    );
    assert_eq!(
        eval(&Expr::cast(ExprType::Fixed(DType::I8), X()), &Column::F64(vec![f64::INFINITY]), IntMode::Python),
        Err(raise(ErrorKind::OverflowError, "cannot convert float infinity to integer"))
    );
    assert_eq!(
        eval(&Expr::cast(ExprType::Bool, X()), &Column::F64(vec![0.0, f64::NAN, -0.0]), IntMode::Python),
        Ok(Column::Bool(vec![false, true, false]))
    );
    assert_eq!(
        eval(&Expr::cast(ExprType::Fixed(DType::F32), X()), &Column::I64(vec![16777217]), IntMode::Python),
        Ok(Column::F32(vec![16777216.0]))
    );
    assert_eq!(
        eval(&Expr::cast(ExprType::Float, X()), &Column::I32(vec![-4]), IntMode::Fixed),
        Ok(Column::F64(vec![-4.0]))
    );
    let e = Expr::cast(ExprType::Fixed(DType::I8), X());
    assert_eq!(e.to_string(), "X.cast(i8)");
    assert_eq!(e.result_type(ExprType::Int), Ok(ExprType::Fixed(DType::I8)));
}

#[test]
fn float_power_errors() {
    let col = Column::F64(vec![2.0, -8.0]);
    assert_eq!(eval(&bin(Pow, X(), c(2i64)), &col, IntMode::Python), Ok(Column::F64(vec![4.0, 64.0])));
    assert_eq!(eval(&bin(Pow, X(), c(0.5)), &col, IntMode::Python), Err(ExprError::Overflow));
    assert_eq!(
        eval(&bin(Pow, X(), c(10000i64)), &col, IntMode::Python),
        Err(raise(ErrorKind::RangeError, "Numerical result out of range"))
    );
    assert_eq!(
        eval(&bin(Pow, c(0i64), bin(Sub, X(), c(3i64))), &Column::I64(vec![2]), IntMode::Python),
        Err(ExprError::Overflow)
    );
    assert_eq!(
        eval(&bin(Pow, bin(Mul, X(), c(0i64)), c(-1i64)), &Column::I64(vec![2]), IntMode::Python),
        Err(raise(ErrorKind::ZeroDivisionError, "zero to a negative power"))
    );
}

#[test]
fn types_render_and_limits() {
    let e = bin(Lt, Expr::unary(UnaryOp::Abs, bin(TrueDiv, X(), c(2i64))), c(1.5));
    assert_eq!(e.to_string(), "(abs((X / 2)) < 1.5)");
    assert_eq!(e.result_type(ExprType::Int), Ok(ExprType::Bool));
    assert_eq!(bin(FloorDiv, X(), c(2i64)).result_type(ExprType::Bool), Ok(ExprType::Int));
    assert_eq!(bin(Pow, X(), c(-2i64)).result_type(ExprType::Int), Ok(ExprType::Float));
    assert_eq!(bin(Add, X(), c(1i64)).result_type(ExprType::Fixed(DType::U8)), Ok(ExprType::Fixed(DType::U8)));
    assert_eq!(Expr::unary(UnaryOp::IsNan, X()).to_string(), "X.is_nan()");
    assert_eq!(Expr::unary(UnaryOp::Neg, c(-1i64)).to_string(), "(--1)");
    assert_eq!(
        eval(&Expr::unary(UnaryOp::IsNan, X()), &Column::F64(vec![f64::NAN, 1.0]), IntMode::Python),
        Ok(Column::Bool(vec![true, false]))
    );
    assert_eq!(eval(&X(), &Column::I32(vec![7]), IntMode::Python), Ok(Column::I64(vec![7])));
    assert_eq!(eval(&c(5i64), &Column::I8(vec![1, 2]), IntMode::Fixed), Ok(Column::I64(vec![5, 5])));
    let mut deep = X();
    for _ in 0..MAX_DEPTH {
        deep = bin(Add, deep, c(1i64));
    }
    assert_eq!(
        kind_of(&eval(&deep, &Column::I64(vec![1]), IntMode::Python).unwrap_err()),
        Some(ErrorKind::RecursionError)
    );
    assert_eq!(truthy(&Column::F64(vec![0.0, f64::NAN])), vec![false, true]);
}

#[test]
fn large_columns_split_across_workers() {
    let n = 300_000;
    let col = Column::I64((0..n).collect());
    let e = bin(Add, bin(Mul, X(), c(3i64)), c(1i64));
    let small = Tuning { chunk: 100, parallel_min: 1000 };
    let expected = Column::I64((0..n).map(|x| x * 3 + 1).collect());
    assert_eq!(exec::evaluate(&e, &col, IntMode::Python, &small), Ok(expected.clone()));
    assert_eq!(eval(&e, &col, IntMode::Python), Ok(expected));
    // The reported failure is the earliest, whichever worker finds it first.
    let mut v: Vec<i64> = vec![1; n as usize];
    v[250_000] = 0;
    v[260_000] = -1;
    v[123_456] = -2;
    let e = bin(Add, bin(FloorDiv, c(1i64), X()), bin(LShift, c(1i64), bin(Add, X(), c(1i64))));
    let col = Column::I64(v);
    let err = exec::evaluate(&e, &col, IntMode::Python, &small).unwrap_err();
    assert_eq!(err, raise(ErrorKind::ValueError, "negative shift count"));
}

// ---------------------------------------------------------------------------------------
// Reference: a direct per-element interpreter over dynamic Python-like values.

#[derive(Clone, Debug)]
enum V {
    B(bool),
    I(i128),
    F(f64),
    S(String),
    /// A fixed-width integer.
    X(DType, i128),
}

#[derive(Clone, Debug, PartialEq)]
enum Event {
    Fallback,
    Raise(ErrorKind, String),
}

type R = Result<V, Event>;

fn type_err(msg: String) -> Event {
    Event::Raise(ErrorKind::TypeError, msg)
}

fn name(v: &V) -> &'static str {
    match v {
        V::B(_) => "bool",
        V::I(_) => "int",
        V::F(_) => "float",
        V::S(_) => "str",
        V::X(d, _) => d.name(),
    }
}

fn int(v: i128) -> R {
    if i64::try_from(v).is_ok() { Ok(V::I(v)) } else { Err(Event::Fallback) }
}

fn fixed(d: DType, v: i128) -> R {
    let (lo, hi) = d.int_range().unwrap();
    if (lo..=hi).contains(&v) { Ok(V::X(d, v)) } else { Err(Event::Raise(ErrorKind::OverflowError, String::new())) }
}

/// `v` reduced to the width of `d` (two's complement).
fn wrap(d: DType, v: i128) -> i128 {
    let bits = d.bits().unwrap();
    let low = v & ((1i128 << bits) - 1);
    if d.is_signed() && low >= 1i128 << (bits - 1) { low - (1i128 << bits) } else { low }
}

fn as_int(v: &V) -> Option<i128> {
    match *v {
        V::B(b) => Some(b as i128),
        V::I(i) | V::X(_, i) => Some(i),
        _ => None,
    }
}

fn floor_div(a: i128, b: i128) -> i128 {
    let q = a / b;
    if a % b != 0 && ((a < 0) != (b < 0)) { q - 1 } else { q }
}

fn ipow(a: i128, mut e: i128) -> Option<i128> {
    let (mut acc, mut base) = (1i128, a);
    while e > 0 {
        if e & 1 == 1 {
            acc = acc.checked_mul(base)?;
        }
        e >>= 1;
        if e > 0 {
            base = base.checked_mul(base)?;
        }
    }
    Some(acc)
}

fn fpow(a: f64, b: f64) -> R {
    kernels::float_pow(a, b).map(V::F).map_err(|f| match f {
        Fault::ZeroToNegative => Event::Raise(ErrorKind::ZeroDivisionError, "zero to a negative power".into()),
        Fault::Range => Event::Raise(ErrorKind::RangeError, "Numerical result out of range".into()),
        _ => Event::Fallback,
    })
}

thread_local! {
    /// Set when the reference evaluated an operation the engine may legitimately decline
    /// (returning `Overflow` so the binding falls back to exact per-element evaluation).
    static MAY_DECLINE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn zero_div() -> Event {
    Event::Raise(ErrorKind::ZeroDivisionError, "division by zero".into())
}

/// Python's exact number comparison (independent of the kernels).
fn num_cmp(a: &V, b: &V) -> Option<Ordering> {
    match (as_int(a), as_int(b), a, b) {
        (Some(x), Some(y), ..) => Some(x.cmp(&y)),
        (Some(x), None, _, V::F(f)) => int_vs_float(x, *f),
        (None, Some(y), V::F(f), _) => int_vs_float(y, *f).map(Ordering::reverse),
        (_, _, V::F(x), V::F(y)) => x.partial_cmp(y),
        _ => unreachable!(),
    }
}

fn int_vs_float(x: i128, f: f64) -> Option<Ordering> {
    if f.is_nan() {
        return None;
    }
    if f.is_infinite() {
        return Some(if f > 0.0 { Ordering::Less } else { Ordering::Greater });
    }
    let fl = f.floor();
    if fl.abs() > 1e30 {
        return Some(if fl > 0.0 { Ordering::Less } else { Ordering::Greater });
    }
    match x.cmp(&(fl as i128)) {
        Ordering::Equal if f > fl => Some(Ordering::Less),
        o => Some(o),
    }
}

fn unsupported(op: BinaryOp, a: &V, b: &V) -> Event {
    let sym = if op == Pow { "** or pow()" } else { op.symbol() };
    type_err(format!("unsupported operand type(s) for {sym}: '{}' and '{}'", name(a), name(b)))
}

fn reference(e: &Expr, x: &V) -> R {
    match e.node() {
        Node::Element => Ok(x.clone()),
        Node::Const(v) => Ok(match v {
            Value::Bool(b) => V::B(*b),
            Value::F64(f) => V::F(*f),
            Value::Str(s) => V::S(s.to_string()),
            other => V::I(other.as_i128().unwrap()),
        }),
        Node::Unary(op, a) => {
            let a = reference(a, x)?;
            ref_unary(*op, a)
        }
        Node::Binary(op, l, r) => {
            let a = reference(l, x)?;
            let b = reference(r, x)?;
            ref_binary(*op, a, b, !r.has_element())
        }
        Node::Cast(t, a) => {
            let a = reference(a, x)?;
            ref_cast(*t, a)
        }
    }
}

fn ref_unary(op: UnaryOp, a: V) -> R {
    let sym = match op {
        UnaryOp::Neg => "unary -",
        UnaryOp::Pos => "unary +",
        UnaryOp::Invert => "unary ~",
        UnaryOp::Abs => "abs()",
        UnaryOp::IsNan => "",
    };
    match (op, a) {
        (UnaryOp::IsNan, V::S(_)) => Err(type_err("must be real number, not str".into())),
        (_, V::S(_)) => Err(type_err(format!("bad operand type for {sym}: 'str'"))),
        (UnaryOp::IsNan, V::F(f)) => Ok(V::B(f.is_nan())),
        (UnaryOp::IsNan, _) => Ok(V::B(false)),
        (UnaryOp::Invert, V::F(_)) => Err(type_err("bad operand type for unary ~: 'float'".into())),
        (UnaryOp::Invert, V::B(b)) => Ok(V::B(!b)),
        (UnaryOp::Invert, V::X(d, v)) if !d.is_signed() => Ok(V::X(d, d.int_range().unwrap().1 - v)),
        (UnaryOp::Invert, V::X(d, v)) => fixed(d, -v - 1),
        (UnaryOp::Invert, a) => int(-as_int(&a).unwrap() - 1),
        (UnaryOp::Neg, V::F(f)) => Ok(V::F(-f)),
        (UnaryOp::Abs, V::F(f)) => Ok(V::F(f.abs())),
        (UnaryOp::Pos, V::F(f)) => Ok(V::F(f)),
        (UnaryOp::Neg, V::X(d, v)) => fixed(d, -v),
        (UnaryOp::Abs, V::X(d, v)) => fixed(d, v.abs()),
        (UnaryOp::Pos, V::X(d, v)) => Ok(V::X(d, v)),
        (UnaryOp::Neg, a) => int(-as_int(&a).unwrap()),
        (UnaryOp::Abs, a) => int(as_int(&a).unwrap().abs()),
        (UnaryOp::Pos, a) => int(as_int(&a).unwrap()),
    }
}

fn ref_cast(t: ExprType, a: V) -> R {
    match (t, &a) {
        (ExprType::Bool, _) => Ok(V::B(match a {
            V::F(f) => f != 0.0,
            V::S(s) => !s.is_empty(),
            ref other => as_int(other).unwrap() != 0,
        })),
        (ExprType::Float, V::F(f)) => Ok(V::F(*f)),
        (ExprType::Float, other) => Ok(V::F(as_int(other).unwrap() as f64)),
        (_, V::F(f)) if f.is_nan() => {
            Err(Event::Raise(ErrorKind::ValueError, "cannot convert float NaN to integer".into()))
        }
        (_, V::F(f)) if f.is_infinite() => {
            Err(Event::Raise(ErrorKind::OverflowError, "cannot convert float infinity to integer".into()))
        }
        (ExprType::Int, V::F(f)) => int(f.trunc() as i128),
        (ExprType::Int, other) => int(as_int(other).unwrap()),
        (ExprType::Fixed(d), V::F(f)) => fixed(d, f.trunc() as i128),
        (ExprType::Fixed(d), other) => fixed(d, as_int(other).unwrap()),
        _ => unreachable!(),
    }
}

fn ref_binary(op: BinaryOp, a: V, b: V, rhs_constant: bool) -> R {
    if op.is_comparison() {
        let unrelated =
            matches!(a, V::S(_)) != matches!(b, V::S(_)) || matches!((&a, &b), (V::X(x, _), V::X(y, _)) if x != y);
        let ord = if unrelated {
            return match op {
                Eq => Ok(V::B(false)),
                Ne => Ok(V::B(true)),
                _ => Err(type_err(format!(
                    "'{}' not supported between instances of '{}' and '{}'",
                    op.symbol(),
                    name(&a),
                    name(&b)
                ))),
            };
        } else if let (V::S(x), V::S(y)) = (&a, &b) {
            Some(x.cmp(y))
        } else {
            num_cmp(&a, &b)
        };
        let r = match op {
            Eq => ord == Some(Ordering::Equal),
            Ne => ord != Some(Ordering::Equal),
            Lt => ord == Some(Ordering::Less),
            Le => matches!(ord, Some(Ordering::Less | Ordering::Equal)),
            Gt => ord == Some(Ordering::Greater),
            _ => matches!(ord, Some(Ordering::Greater | Ordering::Equal)),
        };
        return Ok(V::B(r));
    }
    let bitwise = matches!(op, BitAnd | BitOr | BitXor | LShift | RShift);
    // Fixed-width arithmetic: Python meaning, range-checked.
    let fixed_dtype = match (&a, &b) {
        (V::X(x, _), V::X(y, _)) if x != y => return Err(unsupported(op, &a, &b)),
        (V::X(d, _), _) | (_, V::X(d, _)) => Some(*d),
        _ => None,
    };
    if let Some(d) = fixed_dtype {
        if matches!(a, V::F(_) | V::S(_)) || matches!(b, V::F(_) | V::S(_)) {
            return Err(unsupported(op, &a, &b));
        }
        let (x, y) = (as_int(&a).unwrap(), as_int(&b).unwrap());
        fixed(d, x)?;
        // A shift count is not converted to the type (it is checked against the width).
        if !matches!(op, LShift | RShift) || matches!(b, V::X(..)) {
            fixed(d, y)?;
        }
        let neg_shift = || Err(Event::Raise(ErrorKind::ValueError, "negative shift count".into()));
        let bits = i128::from(d.bits().unwrap());
        return match op {
            TrueDiv if y == 0 => Err(zero_div()),
            TrueDiv => Ok(V::F(kernels::int_truediv(x as i64, y as i64).0)),
            FloorDiv | Mod if y == 0 => Err(zero_div()),
            FloorDiv => fixed(d, floor_div(x, y)),
            Mod => fixed(d, x - floor_div(x, y) * y),
            Pow if y < 0 => {
                Err(Event::Raise(ErrorKind::ValueError, "Integers to negative integer powers are not allowed.".into()))
            }
            Pow => ipow(x, y).map_or(Err(Event::Raise(ErrorKind::OverflowError, String::new())), |v| fixed(d, v)),
            LShift | RShift if y < 0 => neg_shift(),
            LShift | RShift if y >= bits => Err(Event::Raise(
                ErrorKind::OverflowError,
                format!("shift count out of range for {d} (must be < {bits})"),
            )),
            // Bits shifted out are dropped: keep the low `bits` bits, two's complement.
            LShift => Ok(V::X(d, wrap(d, x << y))),
            RShift => Ok(V::X(d, x >> y)),
            BitAnd => fixed(d, x & y),
            BitOr => fixed(d, x | y),
            BitXor => fixed(d, x ^ y),
            Add => fixed(d, x + y),
            Sub => fixed(d, x - y),
            _ => fixed(d, x * y),
        };
    }
    if matches!(a, V::S(_)) || matches!(b, V::S(_)) {
        return ref_text(op, a, b);
    }
    let float = matches!(a, V::F(_)) || matches!(b, V::F(_));
    if bitwise && float {
        return Err(unsupported(op, &a, &b));
    }
    if let (V::B(x), V::B(y), BitAnd | BitOr | BitXor) = (&a, &b, op) {
        return Ok(V::B(match op {
            BitAnd => x & y,
            BitOr => x | y,
            _ => x ^ y,
        }));
    }
    let f = |v: &V| match v {
        V::F(f) => *f,
        other => as_int(other).unwrap() as f64,
    };
    let negative_constant_exponent = rhs_constant && as_int(&b).is_some_and(|e| e < 0);
    if float || (op == Pow && negative_constant_exponent) {
        let (x, y) = (f(&a), f(&b));
        return match op {
            Add => Ok(V::F(x + y)),
            Sub => Ok(V::F(x - y)),
            Mul => Ok(V::F(x * y)),
            TrueDiv | FloorDiv | Mod if y == 0.0 => Err(zero_div()),
            TrueDiv => Ok(V::F(x / y)),
            FloorDiv => Ok(V::F(kernels::float_floordiv(x, y))),
            Mod => Ok(V::F(kernels::float_mod(x, y))),
            _ => fpow(x, y),
        };
    }
    let (x, y) = (as_int(&a).unwrap(), as_int(&b).unwrap());
    match op {
        Add => int(x + y),
        Sub => int(x - y),
        Mul => int(x * y),
        TrueDiv | FloorDiv | Mod if y == 0 => Err(zero_div()),
        TrueDiv => Ok(V::F(kernels::int_truediv(x as i64, y as i64).0)),
        FloorDiv => int(floor_div(x, y)),
        Mod => int(x - floor_div(x, y) * y),
        // CPython's long_pow hands negative exponents to float_pow(float(x), float(y)). The
        // engine may compute that or decline (exact per-element fallback); both are sound.
        Pow if y < 0 => {
            MAY_DECLINE.set(true);
            fpow(x as f64, y as f64)
        }
        Pow => ipow(x, y).map_or(Err(Event::Fallback), int),
        LShift | RShift if y < 0 => Err(Event::Raise(ErrorKind::ValueError, "negative shift count".into())),
        LShift if x == 0 => int(0),
        LShift if y >= 64 => Err(Event::Fallback),
        LShift => int(x << y),
        RShift => int(x >> y.min(127)),
        BitAnd => int(x & y),
        BitOr => int(x | y),
        _ => int(x ^ y),
    }
}

fn ref_text(op: BinaryOp, a: V, b: V) -> R {
    match (op, &a, &b) {
        (Add, V::S(x), V::S(y)) => Ok(V::S(format!("{x}{y}"))),
        (Add, V::S(_), other) => Err(type_err(format!("can only concatenate str (not \"{}\") to str", name(other)))),
        (Mul, V::S(s), n) | (Mul, n, V::S(s)) => match as_int(n) {
            Some(k) => Ok(V::S(s.repeat(k.max(0) as usize))),
            None => Err(type_err(format!("can't multiply sequence by non-int of type '{}'", name(n)))),
        },
        (Mod, V::S(_), _) => Err(Event::Fallback),
        _ => Err(unsupported(op, &a, &b)),
    }
}

fn to_v(col: &Column, i: usize, mode: IntMode) -> V {
    match (col.get(i).unwrap(), mode) {
        (Value::Bool(b), _) => V::B(b),
        (Value::F64(f), _) => V::F(f),
        (Value::Str(s), _) => V::S(s.to_string()),
        (v, IntMode::Python) => V::I(v.as_i128().unwrap()),
        (v, IntMode::Fixed) => V::X(v.dtype(), v.as_i128().unwrap()),
    }
}

fn same(v: &V, got: &Value) -> bool {
    match (v, got) {
        (V::B(a), Value::Bool(b)) => a == b,
        (V::I(a), Value::I64(b)) => *a == *b as i128,
        (V::F(a), Value::F64(b)) => a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan()),
        (V::S(a), Value::Str(b)) => a == b.as_str(),
        (V::X(d, a), b) => b.dtype() == *d && b.as_i128() == Some(*a),
        _ => false,
    }
}

/// Compare the engine with the reference on one column.
fn check(e: &Expr, col: &Column, mode: IntMode) -> Result<(), TestCaseError> {
    let tuning = Tuning { chunk: 7, parallel_min: 40 };
    let got = exec::evaluate(e, col, mode, &tuning);
    let mut expected = Vec::new();
    let mut event = None;
    MAY_DECLINE.set(false);
    for i in 0..col.len() {
        match reference(e, &to_v(col, i, mode)) {
            Ok(v) => expected.push(v),
            Err(ev) => {
                event = Some(ev);
                break;
            }
        }
    }
    match (got, event) {
        (Ok(out), None) => {
            prop_assert_eq!(out.len(), expected.len());
            for (i, v) in expected.iter().enumerate() {
                let g = out.get(i).unwrap();
                prop_assert!(same(v, &g), "element {}: expected {:?}, got {:?} for {} on {:?}", i, v, g, e, col);
            }
        }
        (Err(ExprError::Overflow), Some(Event::Fallback)) => {}
        (Err(ExprError::Overflow), _) if MAY_DECLINE.get() => {}
        (Err(ExprError::Raise { kind, message }), Some(Event::Raise(k, m))) => {
            prop_assert_eq!(kind, k, "{} on {:?}: {}", e, col, message);
            if k != ErrorKind::OverflowError {
                prop_assert_eq!(message, m);
            }
        }
        (got, event) => prop_assert!(false, "{} on {:?}: engine {:?}, reference {:?}", e, col, got, event),
    }
    Ok(())
}

fn arb_expr(fixed_casts: bool) -> impl Strategy<Value = Arc<Expr>> {
    let leaf = prop_oneof![
        4 => Just(()).prop_map(|_| Expr::element()),
        2 => (-6i64..=6).prop_map(Expr::constant),
        1 => prop_oneof![Just(i64::MAX), Just(i64::MIN), Just(1i64 << 32), Just(200i64), Just(-129i64)].prop_map(Expr::constant),
        1 => prop_oneof![
            Just(0.0), Just(-0.0), Just(1.5), Just(-2.5), Just(0.1), Just(f64::NAN),
            Just(f64::INFINITY), Just(-1e300), Just(4.0)
        ].prop_map(Expr::constant),
        1 => any::<bool>().prop_map(Expr::constant),
    ];
    let unary = prop_oneof![
        Just(UnaryOp::Neg),
        Just(UnaryOp::Pos),
        Just(UnaryOp::Abs),
        Just(UnaryOp::Invert),
        Just(UnaryOp::IsNan)
    ];
    let binary = prop::sample::select(vec![
        Add, Sub, Mul, TrueDiv, FloorDiv, Mod, Pow, LShift, RShift, BitAnd, BitOr, BitXor, Eq, Ne, Lt, Le, Gt, Ge,
    ]);
    let mut casts = vec![ExprType::Int, ExprType::Float, ExprType::Bool];
    if fixed_casts {
        casts.extend([ExprType::Fixed(DType::I8), ExprType::Fixed(DType::U8), ExprType::Fixed(DType::I64)]);
    }
    let cast = prop::sample::select(casts);
    leaf.prop_recursive(4, 24, 2, move |inner| {
        prop_oneof![
            1 => (unary.clone(), inner.clone()).prop_map(|(op, a)| Expr::unary(op, a)),
            3 => (binary.clone(), inner.clone(), inner.clone()).prop_map(|(op, a, b)| Expr::binary(op, a, b)),
            1 => (cast.clone(), inner).prop_map(|(t, a)| Expr::cast(t, a)),
        ]
    })
}

fn arb_ints() -> impl Strategy<Value = Vec<i64>> {
    prop::collection::vec(
        prop_oneof![4 => -20i64..=20, 1 => any::<i64>(), 1 => prop_oneof![Just(i64::MAX), Just(i64::MIN), Just(0)]],
        1..120,
    )
}

fn arb_floats() -> impl Strategy<Value = Vec<f64>> {
    prop::collection::vec(
        prop_oneof![
            4 => (-40i32..=40).prop_map(|x| f64::from(x) / 4.0),
            1 => any::<f64>(),
            1 => prop_oneof![Just(f64::NAN), Just(f64::INFINITY), Just(f64::NEG_INFINITY), Just(-0.0), Just(1e308)]
        ],
        1..120,
    )
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 2000, ..ProptestConfig::default() })]

    #[test]
    fn python_ints_match_reference(e in arb_expr(true), data in arb_ints()) {
        check(&e, &Column::I64(data), IntMode::Python)?;
    }

    #[test]
    fn python_floats_match_reference(e in arb_expr(true), data in arb_floats()) {
        check(&e, &Column::F64(data), IntMode::Python)?;
    }

    #[test]
    fn python_bools_match_reference(e in arb_expr(true), data in prop::collection::vec(any::<bool>(), 1..120)) {
        check(&e, &Column::Bool(data), IntMode::Python)?;
    }

    #[test]
    fn fixed_i8_match_reference(e in arb_expr(false), data in prop::collection::vec(any::<i8>(), 1..120)) {
        check(&e, &Column::I8(data), IntMode::Fixed)?;
    }

    #[test]
    fn fixed_u8_match_reference(e in arb_expr(false), data in prop::collection::vec(any::<u8>(), 1..120)) {
        check(&e, &Column::U8(data), IntMode::Fixed)?;
    }

    #[test]
    fn fixed_i64_match_reference(e in arb_expr(false), data in arb_ints()) {
        check(&e, &Column::I64(data), IntMode::Fixed)?;
    }

    #[test]
    fn strings_match_reference(
        op in prop::sample::select(vec![Add, Mul, Eq, Ne, Lt, Ge, Sub, Mod]),
        rhs in prop_oneof![
            "[a-c]{0,3}".prop_map(|s| Expr::constant(s.as_str())),
            (-2i64..4).prop_map(Expr::constant),
            Just(Expr::element()),
            Just(Expr::constant(1.5)),
        ],
        flip in any::<bool>(),
        data in prop::collection::vec("[a-c]{0,3}", 1..60),
    ) {
        let e = if flip { Expr::binary(op, rhs, Expr::element()) } else { Expr::binary(op, Expr::element(), rhs) };
        let col = Column::Str(data.iter().map(CompactString::from).collect());
        check(&e, &col, IntMode::Python)?;
    }
}
