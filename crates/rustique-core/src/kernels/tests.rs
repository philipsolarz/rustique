use std::cmp::Ordering::{self, Equal, Greater, Less};

use compact_str::CompactString;
use proptest::prelude::*;

use super::*;

// ---------------------------------------------------------------------------------------
// Reference model: straightforward, per-element, independent of the kernels' fast paths
// ---------------------------------------------------------------------------------------

enum RefNum {
    Int(i128),
    Float(f64),
}

fn ref_num(v: &Value) -> Option<RefNum> {
    Some(match *v {
        Value::Bool(x) => RefNum::Int(x as i128),
        Value::I8(x) => RefNum::Int(x.into()),
        Value::I16(x) => RefNum::Int(x.into()),
        Value::I32(x) => RefNum::Int(x.into()),
        Value::I64(x) => RefNum::Int(x.into()),
        Value::U8(x) => RefNum::Int(x.into()),
        Value::U16(x) => RefNum::Int(x.into()),
        Value::U32(x) => RefNum::Int(x.into()),
        Value::U64(x) => RefNum::Int(x.into()),
        Value::F32(x) => RefNum::Float(x.into()),
        Value::F64(x) => RefNum::Float(x),
        Value::Str(_) => return None,
    })
}

/// Exact `i <=> f` from the float's binary expansion `±m·2^e`.
fn ref_int_float_cmp(i: i128, f: f64) -> Option<Ordering> {
    if f.is_nan() {
        return None;
    }
    if f.is_infinite() {
        return Some(if f > 0.0 { Less } else { Greater });
    }
    let bits = f.to_bits();
    let negative = bits >> 63 == 1;
    let exp = ((bits >> 52) & 0x7ff) as i32;
    let frac = bits & ((1 << 52) - 1);
    let (m, e) = if exp == 0 { (frac, -1074) } else { (frac | 1 << 52, exp - 1075) };
    if m == 0 {
        return Some(i.cmp(&0));
    }
    if negative != (i < 0) {
        return Some(if negative { Greater } else { Less });
    }
    let a = i.unsigned_abs();
    let magnitude = if e >= 0 {
        if e > 70 { Less } else { a.cmp(&((m as u128) << e)) }
    } else if -e >= 64 {
        if a == 0 { Less } else { Greater }
    } else {
        (a << -e).cmp(&(m as u128))
    };
    Some(if negative { magnitude.reverse() } else { magnitude })
}

/// Python `a <=> b`: `Err` for `TypeError`, `Ok(None)` when unordered.
fn ref_cmp(a: &Value, b: &Value) -> Result<Option<Ordering>, ()> {
    if let (Value::Str(x), Value::Str(y)) = (a, b) {
        return Ok(Some(x.as_bytes().cmp(y.as_bytes())));
    }
    Ok(match (ref_num(a).ok_or(())?, ref_num(b).ok_or(())?) {
        (RefNum::Int(x), RefNum::Int(y)) => Some(x.cmp(&y)),
        (RefNum::Int(i), RefNum::Float(f)) => ref_int_float_cmp(i, f),
        (RefNum::Float(f), RefNum::Int(i)) => ref_int_float_cmp(i, f).map(Ordering::reverse),
        (RefNum::Float(x), RefNum::Float(y)) => x.partial_cmp(&y),
    })
}

fn is_nan(v: &Value) -> bool {
    matches!(ref_num(v), Some(RefNum::Float(f)) if f.is_nan())
}

/// Kernel equality: Python `==`, with NaN equal to NaN.
fn ref_eq(a: &Value, b: &Value) -> bool {
    (is_nan(a) && is_nan(b)) || ref_cmp(a, b) == Ok(Some(Equal))
}

/// Distinguishes `-0.0` from `0.0` and NaN payloads, unlike `Value: PartialEq`.
fn fp(v: &Value) -> String {
    match *v {
        Value::F32(x) => format!("f32:{:08x}", x.to_bits()),
        Value::F64(x) => format!("f64:{:016x}", x.to_bits()),
        ref other => format!("{other:?}"),
    }
}

fn fps(c: &Column) -> Vec<String> {
    c.iter().map(|v| fp(&v)).collect()
}

/// A literal port of CPython's float `sum()` (`builtin_sum_impl` with `cs_add`/`cs_to_double`).
fn ref_float_sum(xs: &[f64]) -> Sum {
    let Some((&first, rest)) = xs.split_first() else { return Sum::Int(0) };
    let (mut hi, mut lo) = (0.0 + first, 0.0);
    for &x in rest {
        let t = hi + x;
        if hi.abs() >= x.abs() {
            lo += (hi - t) + x;
        } else {
            lo += (x - t) + hi;
        }
        hi = t;
    }
    Sum::Float(if lo != 0.0 && lo.is_finite() { hi + lo } else { hi })
}

fn ref_sum(c: &Column) -> Result<Sum, KernelError> {
    match c.dtype() {
        DType::Str if !c.is_empty() => Err(KernelError::Unsupported { op: "sum", dtype: DType::Str }),
        DType::Str => Ok(Sum::Int(0)),
        DType::F32 | DType::F64 => Ok(ref_float_sum(&c.iter().map(|v| v.as_f64().unwrap()).collect::<Vec<_>>())),
        _ => Ok(Sum::Int(c.iter().map(|v| v.as_i128().unwrap()).sum())),
    }
}

fn same_sum(a: &Result<Sum, KernelError>, b: &Result<Sum, KernelError>) -> bool {
    match (a, b) {
        (Ok(Sum::Float(x)), Ok(Sum::Float(y))) => x.to_bits() == y.to_bits() || (x.is_nan() && y.is_nan()),
        _ => a == b,
    }
}

/// CPython's `min_max` loop.
fn ref_extreme(c: &Column, max: bool) -> Option<usize> {
    let vals: Vec<Value> = c.iter().collect();
    let mut best = 0;
    for i in 1..vals.len() {
        let want = if max { Greater } else { Less };
        if ref_cmp(&vals[i], &vals[best]) == Ok(Some(want)) {
            best = i;
        }
    }
    (!vals.is_empty()).then_some(best)
}

/// Stable sort of positions by Python `<`, NaNs last (in original order) in both directions.
fn ref_argsort(c: &Column, reverse: bool) -> Vec<usize> {
    let vals: Vec<Value> = c.iter().collect();
    let mut order: Vec<usize> = (0..vals.len()).collect();
    order.sort_by(|&i, &j| match (is_nan(&vals[i]), is_nan(&vals[j])) {
        (true, true) => Equal,
        (true, false) => Greater,
        (false, true) => Less,
        (false, false) => {
            let o = ref_cmp(&vals[i], &vals[j]).unwrap().unwrap();
            if reverse { o.reverse() } else { o }
        }
    });
    order
}

fn ref_find(c: &Column, needle: &Value, start: usize, stop: usize) -> Option<usize> {
    (start..stop.min(c.len())).find(|&i| ref_eq(&c.get(i).unwrap(), needle))
}

fn ref_lex(a: &Column, b: &Column, op: CmpOp) -> Option<bool> {
    for i in 0..a.len().min(b.len()) {
        let (x, y) = (a.get(i).unwrap(), b.get(i).unwrap());
        if !ref_eq(&x, &y) {
            return ref_cmp(&x, &y).ok().map(|o| op.holds(o));
        }
    }
    Some(op.holds(Some(a.len().cmp(&b.len()))))
}

// ---------------------------------------------------------------------------------------
// Generators: numbers clustered around dtype and float-precision boundaries
// ---------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
enum Src {
    I(i128),
    F(f64),
}

const EDGE_INTS: [i128; 26] = [
    0,
    1,
    -1,
    2,
    127,
    128,
    -129,
    255,
    256,
    32767,
    65535,
    65536,
    (1 << 24) + 1,
    (1 << 31) - 1,
    1 << 31,
    1 << 32,
    (1 << 53) - 1,
    1 << 53,
    (1 << 53) + 1,
    i64::MAX as i128,
    i64::MIN as i128,
    1 << 63,
    (1 << 63) + 1,
    u64::MAX as i128,
    (1 << 64) - 2048,
    -(1 << 53) - 1,
];

const EDGE_FLOATS: [f64; 22] = [
    0.0,
    -0.0,
    f64::NAN,
    -f64::NAN,
    f64::INFINITY,
    f64::NEG_INFINITY,
    0.5,
    1.0,
    -1.0,
    1.5,
    16777216.0,
    16777217.0,
    9007199254740992.0,
    9007199254740994.0,
    9223372036854775808.0,
    -9223372036854775808.0,
    18446744073709551616.0,
    1e308,
    -1e308,
    5e-324,
    3.4028234663852886e38,
    1e-45,
];

fn arb_src() -> impl Strategy<Value = Src> {
    prop_oneof![
        3 => (-3i128..4).prop_map(Src::I),
        3 => prop::sample::select(&EDGE_INTS[..]).prop_map(Src::I),
        3 => prop::sample::select(&EDGE_FLOATS[..]).prop_map(Src::F),
        1 => any::<i64>().prop_map(|x| Src::I(x.into())),
        1 => any::<u64>().prop_map(|x| Src::I(x.into())),
        1 => any::<f64>().prop_map(Src::F),
        2 => (-8i32..8).prop_map(|x| Src::F(x as f64 * 0.5)),
    ]
}

fn arb_dtype() -> impl Strategy<Value = DType> {
    prop::sample::select(&DType::ALL[..])
}

const WORDS: [&str; 8] = ["", "a", "b", "ab", "é", "z", "\u{ffff}", "😀"];

/// Convert like Rust `as` casts (wrapping/saturating), so equal-valued pairs across dtypes
/// are common while out-of-range values still produce some element.
fn cast(dtype: DType, src: Src) -> Value {
    let (i, f) = match src {
        Src::I(i) => (i, i as f64),
        Src::F(f) => (f as i128, f),
    };
    let float_source = matches!(src, Src::F(_));
    match dtype {
        DType::Bool => Value::Bool(if float_source { f != 0.0 } else { i != 0 }),
        DType::I8 => Value::I8(if float_source { f as i8 } else { i as i8 }),
        DType::I16 => Value::I16(if float_source { f as i16 } else { i as i16 }),
        DType::I32 => Value::I32(if float_source { f as i32 } else { i as i32 }),
        DType::I64 => Value::I64(if float_source { f as i64 } else { i as i64 }),
        DType::U8 => Value::U8(if float_source { f as u8 } else { i as u8 }),
        DType::U16 => Value::U16(if float_source { f as u16 } else { i as u16 }),
        DType::U32 => Value::U32(if float_source { f as u32 } else { i as u32 }),
        DType::U64 => Value::U64(if float_source { f as u64 } else { i as u64 }),
        DType::F32 => Value::F32(if float_source { f as f32 } else { i as f32 }),
        DType::F64 => Value::F64(f),
        DType::Str => Value::from(WORDS[i.rem_euclid(WORDS.len() as i128) as usize]),
    }
}

fn column(dtype: DType, srcs: &[Src]) -> Column {
    Column::from_values(dtype, srcs.iter().map(|&s| cast(dtype, s))).unwrap()
}

fn arb_column(max_len: usize) -> impl Strategy<Value = Column> {
    (arb_dtype(), prop::collection::vec(arb_src(), 0..max_len)).prop_map(|(d, xs)| column(d, &xs))
}

/// Two columns of independent dtypes built from mostly the same numbers.
fn arb_pair() -> impl Strategy<Value = (Column, Column)> {
    let edits = prop::collection::vec((any::<prop::sample::Index>(), arb_src()), 0..3);
    (arb_dtype(), arb_dtype(), prop::collection::vec(arb_src(), 0..10), edits, 0usize..4, arb_src()).prop_map(
        |(da, db, xs, edits, shape, extra)| {
            let mut ys = xs.clone();
            for (at, src) in edits {
                if !ys.is_empty() {
                    let i = at.index(ys.len());
                    ys[i] = src;
                }
            }
            match shape {
                1 => ys.truncate(ys.len() / 2),
                2 => ys.push(extra),
                _ => {}
            }
            (column(da, &xs), column(db, &ys))
        },
    )
}

const OPS: [CmpOp; 4] = [CmpOp::Lt, CmpOp::Le, CmpOp::Gt, CmpOp::Ge];

proptest! {
    #[test]
    fn sum_matches_reference(c in arb_column(64)) {
        prop_assert!(same_sum(&sum(&c), &ref_sum(&c)), "{:?} vs {:?}", sum(&c), ref_sum(&c));
    }

    #[test]
    fn float_sum_matches_cpython_port(xs in prop::collection::vec(
        prop_oneof![any::<f64>(), prop::sample::select(&EDGE_FLOATS[..]), -1e6f64..1e6], 0..200)) {
        let ours = sum(&Column::F64(xs.clone()));
        prop_assert!(same_sum(&ours, &Ok(ref_float_sum(&xs))));
    }

    #[test]
    fn min_max_match_reference(c in arb_column(48)) {
        for max in [false, true] {
            let expected = ref_extreme(&c, max);
            let (value, index) = if max { (super::max(&c), argmax(&c)) } else { (super::min(&c), argmin(&c)) };
            prop_assert_eq!(index, expected);
            prop_assert_eq!(value.map(|v| fp(&v)), expected.map(|i| fp(&c.get(i).unwrap())));
        }
    }

    #[test]
    fn sort_matches_stable_reference(c in arb_column(64), reverse in any::<bool>()) {
        let order = ref_argsort(&c, reverse);
        prop_assert_eq!(&argsort(&c, reverse), &order);
        let mut sorted = c.clone();
        sort(&mut sorted, reverse);
        prop_assert_eq!(fps(&sorted), fps(&c.take(&order)));
    }

    #[test]
    fn find_and_count_match_reference(
        dtype in arb_dtype(),
        xs in prop::collection::vec(arb_src(), 0..40),
        needle_dtype in arb_dtype(),
        needle in arb_src(),
        reuse in any::<Option<prop::sample::Index>>(),
        start in 0usize..45,
        stop in 0usize..45,
    ) {
        // Half the needles come from the column's own sources, cast to another dtype.
        let c = column(dtype, &xs);
        let needle = match reuse {
            Some(at) if !xs.is_empty() => cast(needle_dtype, xs[at.index(xs.len())]),
            _ => cast(needle_dtype, needle),
        };
        prop_assert_eq!(find(&c, &needle, start, stop), ref_find(&c, &needle, start, stop));
        prop_assert_eq!(contains(&c, &needle), ref_find(&c, &needle, 0, c.len()).is_some());
        let expected = (0..c.len()).filter(|&i| ref_eq(&c.get(i).unwrap(), &needle)).count();
        prop_assert_eq!(count(&c, &needle), expected);
    }

    #[test]
    fn comparisons_match_reference((a, b) in arb_pair()) {
        let expected_eq = a.len() == b.len() && ref_lex(&a, &b, CmpOp::Le) == Some(true) && ref_lex(&a, &b, CmpOp::Ge) == Some(true);
        let elementwise = a.len() == b.len() && (0..a.len()).all(|i| ref_eq(&a.get(i).unwrap(), &b.get(i).unwrap()));
        prop_assert_eq!(equal(&a, &b), elementwise);
        if elementwise {
            prop_assert!(expected_eq);
        }
        for op in OPS {
            prop_assert_eq!(try_lex_compare(&a, &b, op), ref_lex(&a, &b, op), "{:?}", op);
            prop_assert_eq!(lex_compare(&a, &b, op), ref_lex(&a, &b, op).unwrap_or(false));
        }
    }

    #[test]
    fn scalar_comparisons_match_reference(da in arb_dtype(), db in arb_dtype(), x in arb_src(), y in arb_src()) {
        let (a, b) = (cast(da, x), cast(db, y));
        let ordering = ref_cmp(&a, &b);
        prop_assert_eq!(value_eq(&a, &b), ordering == Ok(Some(Equal)));
        for op in OPS {
            prop_assert_eq!(value_cmp(&a, &b, op), ordering.ok().map(|o| op.holds(o)));
        }
    }
}

// ---------------------------------------------------------------------------------------
// Edge cases
// ---------------------------------------------------------------------------------------

const TWO53: i64 = 1 << 53;

#[test]
fn int_float_equality_is_exact() {
    let eq = |a: Value, b: Value| {
        value_eq(&a, &b)
            && equal(
                &Column::from_values(a.dtype(), [a.clone()]).unwrap(),
                &Column::from_values(b.dtype(), [b]).unwrap(),
            )
    };
    assert!(eq(Value::I64(TWO53), Value::F64(TWO53 as f64)));
    assert!(!eq(Value::I64(TWO53 + 1), Value::F64(TWO53 as f64)));
    assert!(!eq(Value::I64(i64::MAX), Value::F64(9223372036854775808.0)));
    assert!(eq(Value::I64(i64::MIN), Value::F64(-9223372036854775808.0)));
    assert!(eq(Value::U64(1 << 63), Value::F64(9223372036854775808.0)));
    assert!(!eq(Value::U64(u64::MAX), Value::F64(18446744073709551616.0)));
    assert!(eq(Value::U64(u64::MAX - 2047), Value::F64((u64::MAX - 2047) as f64)));
    assert!(eq(Value::I32(16777217), Value::F64(16777217.0)));
    assert!(!eq(Value::I32(16777217), Value::F32(16777216.0)));
    assert!(eq(Value::Bool(true), Value::I8(1)));
    assert!(eq(Value::Bool(true), Value::F64(1.0)));
    assert!(eq(Value::Bool(false), Value::F32(-0.0)));
    assert!(eq(Value::F64(-0.0), Value::U8(0)));
    assert!(eq(Value::F32(0.1), Value::F64(0.1f32 as f64)));
    assert!(!eq(Value::F32(0.1), Value::F64(0.1)));
    assert!(!eq(Value::Str("1".into()), Value::I64(1)));
    assert!(!eq(Value::F64(f64::NAN), Value::I64(0)));
    assert!(!value_eq(&Value::F64(f64::NAN), &Value::F64(f64::NAN)));
    // Columns treat NaN as one shared object.
    assert!(equal(&Column::F64(vec![f64::NAN]), &Column::F32(vec![f32::NAN])));
}

#[test]
fn int_float_ordering_is_exact() {
    let lt = |a: Value, b: Value| value_cmp(&a, &b, CmpOp::Lt).unwrap();
    assert!(lt(Value::F64(TWO53 as f64), Value::I64(TWO53 + 1)));
    assert!(!lt(Value::I64(TWO53 + 1), Value::F64(TWO53 as f64)));
    assert!(lt(Value::I64(i64::MAX), Value::F64(9223372036854775808.0)));
    assert!(lt(Value::U64(u64::MAX), Value::F64(18446744073709551616.0)));
    assert!(lt(Value::F64(-9223372036854775808.0), Value::I64(i64::MIN + 1)));
    assert!(lt(Value::I8(-3), Value::F64(-2.5)) && lt(Value::F64(-3.5), Value::I8(-3)));
    assert!(lt(Value::U64(u64::MAX), Value::F64(f64::INFINITY)));
    assert!(lt(Value::F64(f64::NEG_INFINITY), Value::I64(i64::MIN)));
    for op in OPS {
        assert!(!value_cmp(&Value::F64(f64::NAN), &Value::I64(1), op).unwrap());
    }
    assert_eq!(value_cmp(&Value::Str("a".into()), &Value::I64(1), CmpOp::Lt), None);
    assert_eq!(value_cmp(&Value::from("é"), &Value::from("z"), CmpOp::Gt), Some(true));
    assert_eq!(value_cmp(&Value::from("\u{ffff}"), &Value::from("😀"), CmpOp::Lt), Some(true));
}

#[test]
fn sum_edge_cases() {
    let float = |c: Column| match sum(&c).unwrap() {
        Sum::Float(x) => x,
        other => panic!("{other:?}"),
    };
    assert_eq!(sum(&Column::F64(vec![])), Ok(Sum::Int(0)));
    assert_eq!(sum(&Column::Str(vec![])), Ok(Sum::Int(0)));
    assert_eq!(sum(&Column::from(vec!["a"])), Err(KernelError::Unsupported { op: "sum", dtype: DType::Str }));
    assert_eq!(
        KernelError::Unsupported { op: "sum", dtype: DType::Str }.to_string(),
        "sum is not supported for str data"
    );
    // `0 + -0.0` is `0.0` in Python, so the implicit start makes the sum positive.
    assert_eq!(float(Column::F64(vec![-0.0, -0.0])).to_bits(), 0);
    assert_eq!(float(Column::F64(vec![0.1; 10])), 1.0);
    assert_eq!(float(Column::F64(vec![1e308, 1e308, -1e308])), f64::INFINITY);
    assert_eq!(float(Column::F64(vec![1e100, 1.0, -1e100, 1e-100])), 1.0);
    assert!(float(Column::F64(vec![f64::INFINITY, f64::NEG_INFINITY])).is_nan());
    assert_eq!(float(Column::F32(vec![0.1; 3])), 3.0 * (0.1f32 as f64));
    assert_eq!(sum(&Column::Bool(vec![true, false, true])), Ok(Sum::Int(2)));
    assert_eq!(sum(&Column::I8(vec![-128; 1000])), Ok(Sum::Int(-128_000)));
    assert_eq!(sum(&Column::U64(vec![u64::MAX; 5])), Ok(Sum::Int(5 * u64::MAX as i128)));
    assert_eq!(sum(&Column::I64(vec![i64::MIN, i64::MIN, -1])), Ok(Sum::Int(2 * i64::MIN as i128 - 1)));
    assert_eq!(sum(&Column::I64(vec![i64::MAX, 1, i64::MIN])), Ok(Sum::Int(0)));
}

#[test]
fn min_max_edge_cases() {
    let bits = |v: Option<Value>| match v {
        Some(Value::F64(x)) => x.to_bits(),
        other => panic!("{other:?}"),
    };
    let c = |xs: &[f64]| Column::F64(xs.to_vec());
    assert_eq!(bits(min(&c(&[0.0, -0.0]))), 0.0f64.to_bits());
    assert_eq!(bits(min(&c(&[-0.0, 0.0]))), (-0.0f64).to_bits());
    assert_eq!(bits(max(&c(&[0.0, -0.0]))), 0.0f64.to_bits());
    assert!(f64::from_bits(bits(min(&c(&[f64::NAN, 1.0])))).is_nan());
    assert_eq!(bits(min(&c(&[1.0, f64::NAN, 0.5]))), 0.5f64.to_bits());
    assert_eq!(bits(max(&c(&[2.0, f64::NAN, 3.0]))), 3.0f64.to_bits());
    assert_eq!(argmax(&c(&[2.0, f64::NAN, 3.0, 3.0])), Some(2));
    assert_eq!(argmin(&c(&[f64::NAN, -1.0])), Some(0));
    assert_eq!(min(&Column::I64(vec![])), None);
    assert_eq!(argmax(&Column::Str(vec![])), None);
    assert_eq!(max(&Column::from(vec!["b", "é", "a"])), Some(Value::from("é")));
    assert_eq!(min(&Column::Bool(vec![true, false])), Some(Value::Bool(false)));
    assert_eq!(max(&Column::U64(vec![1, u64::MAX, 3])), Some(Value::U64(u64::MAX)));
    // A zero extreme found deep inside a long column is still the first zero.
    let mut xs = vec![1.0; 100];
    xs[37] = -0.0;
    xs[90] = 0.0;
    assert_eq!(bits(min(&c(&xs))), (-0.0f64).to_bits());
    assert_eq!(argmin(&c(&xs)), Some(37));
}

#[test]
fn sort_edge_cases() {
    let sorted = |xs: &[f64], reverse: bool| {
        let mut col = Column::F64(xs.to_vec());
        sort(&mut col, reverse);
        fps(&col)
    };
    let f = |xs: &[f64]| fps(&Column::F64(xs.to_vec()));
    let (n, z, nz) = (f64::NAN, 0.0, -0.0);
    assert_eq!(sorted(&[z, nz, 1.0, nz, z], true), f(&[1.0, z, nz, nz, z]));
    assert_eq!(sorted(&[z, nz, 1.0, nz], false), f(&[z, nz, nz, 1.0]));
    assert_eq!(sorted(&[n, 2.0, -n, 1.0], false), f(&[1.0, 2.0, n, -n]));
    assert_eq!(sorted(&[n, 2.0, -n, 1.0], true), f(&[2.0, 1.0, n, -n]));
    assert_eq!(sorted(&[n, n], false), f(&[n, n]));
    assert_eq!(argsort(&Column::F64(vec![n, 1.0, z, nz, 1.0]), true), vec![1, 4, 2, 3, 0]);
    assert_eq!(argsort(&Column::Bool(vec![true, false, true, false]), true), vec![0, 2, 1, 3]);
    assert_eq!(argsort(&Column::from(vec!["b", "a", "b", "a"]), true), vec![0, 2, 1, 3]);

    let mut strs = Column::from(vec!["😀", "z", "\u{ffff}", "é", "", "a"]);
    sort(&mut strs, false);
    assert_eq!(strs, Column::from(vec!["", "a", "z", "é", "\u{ffff}", "😀"]));

    let mut bytes = Column::I8(vec![5, -128, 127, 0, -1, 5]);
    sort(&mut bytes, true);
    assert_eq!(bytes, Column::I8(vec![127, 5, 5, 0, -1, -128]));
    let mut shorts = Column::I16((0..20_000).map(|i| (i * 7919 % 65_536 - 32_768) as i16).collect());
    let mut expected = shorts.as_slice::<i16>().unwrap().to_vec();
    expected.sort();
    sort(&mut shorts, false);
    assert_eq!(shorts.as_slice::<i16>().unwrap(), &expected[..]);
}

#[test]
fn search_edge_cases() {
    let c = Column::F64(vec![1.0, f64::NAN, -0.0, 9007199254740992.0, 1.0]);
    assert_eq!(find(&c, &Value::I64(0), 0, 5), Some(2));
    assert_eq!(find(&c, &Value::F32(f32::NAN), 0, 5), Some(1));
    assert_eq!(find(&c, &Value::I64(TWO53), 0, 5), Some(3));
    assert_eq!(find(&c, &Value::I64(TWO53 + 1), 0, 5), None);
    assert_eq!(find(&c, &Value::Bool(true), 1, 5), Some(4));
    assert_eq!(find(&c, &Value::Bool(true), 1, 4), None);
    assert_eq!(find(&c, &Value::I64(1), 4, usize::MAX), Some(4));
    assert_eq!(find(&c, &Value::I64(1), 5, 3), None);
    assert_eq!(count(&c, &Value::F64(f64::NAN)), 1);
    assert_eq!(count(&c, &Value::U8(1)), 2);
    assert_eq!(count(&Column::I64(vec![0, 1]), &Value::F64(f64::NAN)), 0);
    assert_eq!(count(&Column::Bool(vec![true, false, true]), &Value::F64(1.0)), 2);
    assert_eq!(count(&Column::Bool(vec![true]), &Value::I64(2)), 0);
    assert_eq!(count(&Column::U64(vec![u64::MAX]), &Value::F64(18446744073709551616.0)), 0);
    assert_eq!(count(&Column::F32(vec![16777216.0]), &Value::I32(16777217)), 0);
    let narrow = Column::F32(vec![16777216.0, 0.1, f32::MAX]);
    assert_eq!(count(&narrow, &Value::F64(16777217.0)), 0);
    assert_eq!(count(&narrow, &Value::F64(0.1)), 0);
    assert_eq!(count(&narrow, &Value::F64(0.1f32 as f64)), 1);
    assert_eq!(count(&narrow, &Value::F64(1e300)), 0);
    assert_eq!(find(&narrow, &Value::F64(f32::MAX as f64), 0, 3), Some(2));
    assert_eq!(count(&Column::from(vec!["a", "b", "a"]), &Value::from("a")), 2);
    assert!(!contains(&Column::from(vec!["1"]), &Value::I64(1)));
    assert!(contains(&Column::I16(vec![3, -7]), &Value::F64(-7.0)));
    assert!(!contains(&Column::I16(vec![3, -7]), &Value::F64(-7.5)));
}

#[test]
fn list_ordering_edge_cases() {
    let lt = |a: Column, b: Column| try_lex_compare(&a, &b, CmpOp::Lt);
    assert_eq!(lt(Column::I64(vec![1, 2]), Column::F64(vec![1.0, 2.5])), Some(true));
    assert_eq!(lt(Column::I64(vec![1]), Column::I64(vec![1, 0])), Some(true));
    assert_eq!(lt(Column::I64(vec![]), Column::from(vec!["a"])), Some(true));
    assert_eq!(lt(Column::from(vec!["a"]), Column::I64(vec![])), Some(false));
    assert_eq!(lt(Column::I64(vec![1]), Column::from(vec!["a"])), None);
    assert!(!lex_compare(&Column::I64(vec![1]), &Column::from(vec!["a"]), CmpOp::Ge));
    let nan = Column::F64(vec![f64::NAN]);
    for op in OPS {
        assert!(!lex_compare(&nan, &Column::F64(vec![1.0]), op));
        // NaN equals NaN in the prefix scan, so the lengths decide.
        assert_eq!(lex_compare(&nan, &nan, op), matches!(op, CmpOp::Le | CmpOp::Ge));
    }
    assert!(lex_compare(&Column::F64(vec![-0.0, 1.0]), &Column::I8(vec![0, 2]), CmpOp::Lt));
    assert!(equal(&Column::I64(vec![]), &Column::from(Vec::<&str>::new())));
    assert!(!equal(&Column::I64(vec![1]), &Column::I64(vec![1, 2])));
    let mut long = vec![7u32; 5000];
    let other = long.clone();
    long[4321] = 8;
    assert!(lex_compare(&Column::U32(long), &Column::U32(other), CmpOp::Gt));
    assert_eq!(CmpOp::Le.symbol(), "<=");
}

// ---------------------------------------------------------------------------------------
// Large inputs: the parallel and counting-sort paths
// ---------------------------------------------------------------------------------------

fn noise(n: usize, seed: u64) -> impl Iterator<Item = u64> {
    let mut s = seed;
    std::iter::repeat_with(move || {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        s
    })
    .take(n)
}

#[test]
fn large_integer_kernels() {
    let n = PAR_SCAN_MIN * 2 + 17;
    let xs: Vec<i64> = noise(n, 1).map(|x| x as i64).collect();
    let c = Column::I64(xs.clone());
    assert_eq!(sum(&c), Ok(Sum::Int(xs.iter().map(|&x| x as i128).sum())));
    let us: Vec<u64> = noise(n, 2).collect();
    assert_eq!(sum(&Column::U64(us.clone())), Ok(Sum::Int(us.iter().map(|&x| x as i128).sum())));
    let narrow: Vec<i32> = xs.iter().map(|&x| x as i32).collect();
    assert_eq!(sum(&Column::I32(narrow.clone())), Ok(Sum::Int(narrow.iter().map(|&x| x as i128).sum())));
    assert_eq!(min(&c), xs.iter().min().map(|&x| Value::I64(x)));
    let largest = *xs.iter().max().unwrap();
    assert_eq!(argmax(&c), xs.iter().position(|&x| x == largest));
    let needle = xs[n / 2] & 0xff;
    let bytes = Column::I64(xs.iter().map(|&x| x & 0xff).collect());
    assert_eq!(count(&bytes, &Value::I64(needle)), xs.iter().filter(|&&x| x & 0xff == needle).count());

    for reverse in [false, true] {
        let mut sorted = c.clone();
        sort(&mut sorted, reverse);
        let mut expected = xs.clone();
        expected.sort_by(|a, b| if reverse { b.cmp(a) } else { a.cmp(b) });
        assert_eq!(sorted.as_slice::<i64>().unwrap(), &expected[..]);
        // Many duplicates: indices of equal keys must stay ascending.
        let order = argsort(&bytes, reverse);
        let keys = bytes.as_slice::<i64>().unwrap();
        let mut expected_order: Vec<usize> = (0..n).collect();
        expected_order.sort_by(|&i, &j| if reverse { keys[j].cmp(&keys[i]) } else { keys[i].cmp(&keys[j]) });
        assert_eq!(order, expected_order);
    }
}

#[test]
fn large_float_kernels() {
    let n = PAR_SCAN_MIN + PAR_SORT_MIN + 5;
    let pick = [0.0, -0.0, 1.5, -2.0, f64::NAN, f64::INFINITY];
    let xs: Vec<f64> = noise(n, 3)
        .map(|r| if r % 4 == 0 { pick[(r >> 8) as usize % pick.len()] } else { (r >> 11) as f64 / 1e6 - 4e6 })
        .collect();
    let c = Column::F64(xs.clone());
    assert!(same_sum(&sum(&c), &Ok(ref_float_sum(&xs))));
    for max in [false, true] {
        let mut best = 0;
        for i in 1..n {
            if if max { xs[i] > xs[best] } else { xs[i] < xs[best] } {
                best = i;
            }
        }
        let got = if max { argmax(&c) } else { argmin(&c) };
        assert_eq!(got, Some(best));
    }
    for reverse in [false, true] {
        let mut order: Vec<usize> = (0..n).collect();
        order.sort_by(|&i, &j| match (xs[i].is_nan(), xs[j].is_nan()) {
            (true, true) => Equal,
            (a, b) if a || b => a.cmp(&b),
            _ if reverse => xs[j].partial_cmp(&xs[i]).unwrap(),
            _ => xs[i].partial_cmp(&xs[j]).unwrap(),
        });
        assert_eq!(argsort(&c, reverse), order);
        let mut sorted = c.clone();
        sort(&mut sorted, reverse);
        let got: Vec<u64> = sorted.as_slice::<f64>().unwrap().iter().map(|x| x.to_bits()).collect();
        let expected: Vec<u64> = order.iter().map(|&i| xs[i].to_bits()).collect();
        assert_eq!(got, expected);
    }
    let strs: Vec<CompactString> =
        noise(PAR_SORT_MIN + 3, 4).map(|r| CompactString::from(WORDS[r as usize % WORDS.len()])).collect();
    let order = argsort(&Column::Str(strs.clone()), true);
    let mut expected: Vec<usize> = (0..strs.len()).collect();
    expected.sort_by(|&i, &j| strs[j].cmp(&strs[i]));
    assert_eq!(order, expected);
}
