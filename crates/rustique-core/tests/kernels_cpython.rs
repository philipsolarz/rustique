//! Kernels against results computed by CPython itself.
//!
//! Inputs are rebuilt here from the same xorshift64* stream as `kernels_cpython_gen.py`,
//! which recorded what CPython's `sum`, `min`, `max`, `sorted` and list comparisons return
//! for them. Regenerate the tables below with that script.

use rustique_core::kernels::{self, CmpOp, Sum};
use rustique_core::{Column, Value};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 * 2f64.powi(-53)
    }
    fn pick<T: Copy>(&mut self, items: &[T]) -> T {
        items[(self.next() % items.len() as u64) as usize]
    }
}

const SPECIALS: [f64; 15] = [
    f64::INFINITY,
    f64::NEG_INFINITY,
    f64::NAN,
    0.0,
    -0.0,
    1e308,
    -1e308,
    5e-324,
    -5e-324,
    9007199254740992.0,
    9007199254740994.0,
    1e16,
    -1e16,
    1.0,
    -1.0,
];
const BIGS: [f64; 4] = [1e16, 1e20, 1152921504606846976.0, 1e100];
const ZEROS: [f64; 5] = [0.0, -0.0, 1.0, -1.0, f64::NAN];
const ZEROS_MIN: [f64; 4] = [0.0, -0.0, 1.0, f64::NAN];
const ZEROS_MAX: [f64; 4] = [0.0, -0.0, -1.0, f64::NAN];
const NEGZERO: [f64; 5] = [-0.0, -0.0, -0.0, -5e-324, 5e-324];
const HUGE: [f64; 4] = [1e308, -1e308, 1.7e308, 8.98846567431158e307];

fn wide(r: &mut Rng) -> f64 {
    let negative = r.next() & 1 == 1;
    let m = r.unit();
    let e = (r.next() % 121) as i32 - 60;
    let x = m * 2f64.powi(e);
    if negative { -x } else { x }
}

fn draw(kind: &str, r: &mut Rng) -> f64 {
    match kind {
        "uniform" => r.unit() * 2.0 - 1.0,
        "wide" => wide(r),
        "special" => {
            if r.next().is_multiple_of(8) {
                r.pick(&SPECIALS)
            } else {
                wide(r)
            }
        }
        "cancel" => {
            if r.next().is_multiple_of(2) {
                let big = r.pick(&BIGS);
                if r.next() & 1 == 1 { big } else { -big }
            } else {
                r.unit()
            }
        }
        "intfloat" => ((1i64 << 53) + (r.next() % 16) as i64 - 8) as f64,
        "plusinf" => {
            if r.next().is_multiple_of(50) {
                f64::INFINITY
            } else {
                wide(r)
            }
        }
        "negzero" => r.pick(&NEGZERO),
        "huge" => {
            if r.next().is_multiple_of(4) {
                r.pick(&HUGE)
            } else {
                wide(r)
            }
        }
        "zeros" => r.pick(&ZEROS),
        "zeros_min" => r.pick(&ZEROS_MIN),
        "zeros_max" => r.pick(&ZEROS_MAX),
        "zeros_no_nan" => r.pick(&ZEROS[..4]),
        _ => unreachable!("{kind}"),
    }
}

fn floats(kind: &str, n: usize, seed: u64) -> Vec<f64> {
    let mut r = Rng(seed);
    (0..n).map(|_| draw(kind, &mut r)).collect()
}

/// Order-sensitive hash of the bit patterns (the shift spreads sign bits downward).
fn digest(values: &[f64]) -> u64 {
    values.iter().fold(0xCBF2_9CE4_8422_2325, |h, x| {
        let h = (h ^ x.to_bits()).wrapping_mul(0x100_0000_01B3);
        h ^ h >> 32
    })
}

/// `"NAN"` matches any NaN; otherwise the exact bit pattern (so `-0.0 != 0.0`).
fn assert_bits(actual: f64, expected: &str, context: &str) {
    if expected == "NAN" {
        assert!(actual.is_nan(), "{context}: expected nan, got {actual:?}");
    } else {
        let bits = u64::from_str_radix(expected.trim_start_matches("0x"), 16).unwrap();
        assert_eq!(actual.to_bits(), bits, "{context}: got {actual:?}, expected {:?}", f64::from_bits(bits));
    }
}

fn float_of(value: Option<Value>) -> f64 {
    match value {
        Some(Value::F64(x)) => x,
        other => panic!("expected a float, got {other:?}"),
    }
}

#[test]
fn sum_matches_cpython() {
    for &(kind, n, seed, expected64, expected32) in SUM_CASES {
        let xs = floats(kind, n, seed);
        let Sum::Float(s) = kernels::sum(&Column::F64(xs.clone())).unwrap() else { panic!() };
        assert_bits(s, expected64, &format!("sum f64 {kind} n={n}"));
        if expected32 != "-" {
            let narrow = Column::F32(xs.iter().map(|&x| x as f32).collect());
            let Sum::Float(s) = kernels::sum(&narrow).unwrap() else { panic!() };
            assert_bits(s, expected32, &format!("sum f32 {kind} n={n}"));
        }
    }
}

#[test]
fn min_max_match_cpython() {
    for &(kind, n, seed, expected_min, expected_max) in EXTREME_CASES {
        let column = Column::F64(floats(kind, n, seed));
        assert_bits(float_of(kernels::min(&column)), expected_min, &format!("min {kind} n={n}"));
        assert_bits(float_of(kernels::max(&column)), expected_max, &format!("max {kind} n={n}"));
        let xs = column.as_slice::<f64>().unwrap();
        let at = |i: Option<usize>| xs[i.unwrap()];
        assert_bits(at(kernels::argmin(&column)), expected_min, &format!("argmin {kind} n={n}"));
        assert_bits(at(kernels::argmax(&column)), expected_max, &format!("argmax {kind} n={n}"));
    }
}

#[test]
fn sort_matches_cpython() {
    for &(kind, n, seed, forward, backward) in SORT_CASES {
        let xs = floats(kind, n, seed);
        for (reverse, expected) in [(false, forward), (true, backward)] {
            let mut column = Column::F64(xs.clone());
            kernels::sort(&mut column, reverse);
            assert_eq!(digest(column.as_slice().unwrap()), expected, "sort {kind} n={n} reverse={reverse}");
            let order = kernels::argsort(&Column::F64(xs.clone()), reverse);
            let gathered: Vec<f64> = order.iter().map(|&i| xs[i]).collect();
            assert_eq!(digest(&gathered), expected, "argsort {kind} n={n} reverse={reverse}");
        }
    }
}

const I64_POOL: [i128; 12] = [
    0,
    1,
    -1,
    1 << 53,
    (1 << 53) + 1,
    (1 << 53) - 1,
    i64::MAX as i128,
    i64::MIN as i128,
    i64::MIN as i128 + 1,
    1 << 62,
    -(1 << 53) - 1,
    12345,
];
const U64_POOL: [i128; 9] =
    [0, 1, (1 << 53) + 1, 1 << 63, (1 << 63) + 1, u64::MAX as i128, (1 << 64) - 2048, (1 << 64) - 1024, 1 << 53];
const FLOAT_POOL: [f64; 8] = [
    9007199254740992.0,
    9223372036854775808.0,
    18446744073709551616.0,
    -9223372036854775808.0,
    0.5,
    -0.0,
    f64::INFINITY,
    f64::NAN,
];

/// Mirror of `lex_case` in the generator: an int column and a float column built from it.
fn lex_case(seed: u64) -> (Column, Column) {
    let mut r = Rng(seed);
    let signed = r.next() & 1 == 1;
    let pool: &[i128] = if signed { &I64_POOL } else { &U64_POOL };
    let n = 1 + (r.next() % 5) as usize;
    let ints: Vec<i128> = (0..n).map(|_| r.pick(pool)).collect();
    let mut floats: Vec<f64> = ints
        .iter()
        .map(|&i| match r.next() % 6 {
            0 => r.pick(&FLOAT_POOL),
            1 => r.pick(pool) as f64,
            _ => i as f64,
        })
        .collect();
    match r.next() % 3 {
        1 => floats.truncate((r.next() % (n as u64 + 1)) as usize),
        2 => floats.push(r.pick(pool) as f64),
        _ => {}
    }
    let ints = if signed {
        Column::I64(ints.iter().map(|&i| i as i64).collect())
    } else {
        Column::U64(ints.iter().map(|&i| i as u64).collect())
    };
    (ints, Column::F64(floats))
}

fn mask(a: &Column, b: &Column) -> u8 {
    let ops = [CmpOp::Lt, CmpOp::Le, CmpOp::Gt, CmpOp::Ge];
    let mut m = kernels::equal(a, b) as u8;
    for (k, op) in ops.into_iter().enumerate() {
        m |= (kernels::lex_compare(a, b, op) as u8) << (k + 1);
    }
    m
}

#[test]
fn list_comparison_matches_cpython() {
    for &(seed, forward, backward) in LEX_CASES {
        let (ints, floats) = lex_case(seed);
        assert_eq!(mask(&ints, &floats), forward, "seed {seed}: {ints:?} vs {floats:?}");
        assert_eq!(mask(&floats, &ints), backward, "seed {seed}: {floats:?} vs {ints:?}");
    }
}

// BEGIN GENERATED
// Generated by kernels_cpython_gen.py with CPython 3.15.1.
const SUM_CASES: &[(&str, usize, u64, &str, &str)] = &[
    ("uniform", 1, 1, "0xbfdc0d98da3b4994", "0xbfdc0d98e0000000"),
    ("uniform", 2, 2, "0xbfc89716172c3380", "0xbfc8971630000000"),
    ("uniform", 3, 3, "0xbfd31e0c81e38a20", "0xbfd31e0c71000000"),
    ("uniform", 10, 4, "0x3fec3665c8dc6a72", "0x3fec3665ac000000"),
    ("uniform", 100, 5, "0x4022b5d608905e76", "0x4022b5d609a80000"),
    ("uniform", 1000, 6, "0x40439506657e7924", "0x4043950664e76800"),
    ("uniform", 100000, 7, "0xc0660a134c5053df", "0xc0660a1351a9c920"),
    ("wide", 1, 8, "0x3e68f2dabd4ba938", "0x3e68f2dac0000000"),
    ("wide", 2, 9, "0xc35f36233c1e3c90", "0xc35f362340000000"),
    ("wide", 3, 10, "0xc2cae24f01c014f2", "0xc2cae24effe1be7d"),
    ("wide", 10, 11, "0xc2580180affbe17c", "0xc2580180b44e3b76"),
    ("wide", 100, 12, "0xc3c63ad58d02b40d", "0xc3c63ad58aae400a"),
    ("wide", 1000, 13, "0x437926770ab7ea98", "0x43792676ca70dbd3"),
    ("wide", 100000, 14, "0x43d89f01d74f62dd", "0x43d89f01ed361806"),
    ("special", 1, 15, "0x40db25ef24ebcf8e", "-"),
    ("special", 2, 16, "0xc0370b0e044ba1ee", "-"),
    ("special", 3, 17, "0x7fe1ccf385ebc8a0", "-"),
    ("special", 10, 18, "0x42cc2c7cdbae9bee", "-"),
    ("special", 100, 19, "NAN", "-"),
    ("special", 1000, 20, "NAN", "-"),
    ("special", 100000, 21, "NAN", "-"),
    ("cancel", 1, 22, "0x3fe1a29e05c1b3c7", "-"),
    ("cancel", 2, 23, "0xd4b249ad2594c37d", "-"),
    ("cancel", 3, 24, "0x54b249ad2594c37d", "-"),
    ("cancel", 10, 25, "0x54b249ad2594c37d", "-"),
    ("cancel", 100, 26, "0xd4b249ad2594c37d", "-"),
    ("cancel", 1000, 27, "0xd4d249ad2594c37d", "-"),
    ("cancel", 100000, 28, "0xd52249ad2594c37d", "-"),
    ("intfloat", 1, 29, "0x433ffffffffffffa", "0x4340000000000000"),
    ("intfloat", 2, 30, "0x434ffffffffffffe", "0x4350000000000000"),
    ("intfloat", 3, 31, "0x4357ffffffffffff", "0x4358000000000000"),
    ("intfloat", 10, 32, "0x4373ffffffffffff", "0x4374000000000000"),
    ("intfloat", 100, 33, "0x43a9000000000000", "0x43a9000000000000"),
    ("intfloat", 1000, 34, "0x43df3fffffffffff", "0x43df400000000000"),
    ("intfloat", 100000, 35, "0x44486a0000000000", "0x44486a0000000000"),
    ("plusinf", 1, 36, "0xc0b3d251c9ee2800", "-"),
    ("plusinf", 2, 37, "0xbf882132f515934c", "-"),
    ("plusinf", 3, 38, "0x43a0fe33d93dd8be", "-"),
    ("plusinf", 10, 39, "0x4366fe72c954fd3d", "-"),
    ("plusinf", 100, 40, "0x7ff0000000000000", "-"),
    ("plusinf", 1000, 41, "0x7ff0000000000000", "-"),
    ("plusinf", 100000, 42, "0x7ff0000000000000", "-"),
    ("negzero", 1, 43, "0x0000000000000000", "-"),
    ("negzero", 2, 44, "0x0000000000000002", "-"),
    ("negzero", 3, 45, "0x0000000000000001", "-"),
    ("negzero", 10, 46, "0x8000000000000001", "-"),
    ("negzero", 100, 47, "0x0000000000000000", "-"),
    ("negzero", 1000, 48, "0x8000000000000010", "-"),
    ("negzero", 100000, 49, "0x80000000000000d8", "-"),
    ("huge", 1, 50, "0xc1a7c3511ecfb73c", "-"),
    ("huge", 2, 51, "0xc2b34c368c936ca8", "-"),
    ("huge", 3, 52, "0x4301751b95b01486", "-"),
    ("huge", 10, 53, "0x7ff0000000000000", "-"),
    ("huge", 100, 54, "0x7ff0000000000000", "-"),
    ("huge", 1000, 55, "0x7ff0000000000000", "-"),
    ("huge", 100000, 56, "0x7ff0000000000000", "-"),
];

const EXTREME_CASES: &[(&str, usize, u64, &str, &str)] = &[
    ("zeros", 1, 57, "0x8000000000000000", "0x8000000000000000"),
    ("zeros", 2, 58, "0xbff0000000000000", "0x3ff0000000000000"),
    ("zeros", 5, 59, "0xbff0000000000000", "0xbff0000000000000"),
    ("zeros", 50, 60, "0xbff0000000000000", "0x3ff0000000000000"),
    ("zeros", 1000, 61, "0xbff0000000000000", "0x3ff0000000000000"),
    ("zeros", 300000, 62, "NAN", "NAN"),
    ("zeros_min", 1, 63, "0x0000000000000000", "0x0000000000000000"),
    ("zeros_min", 2, 64, "0x0000000000000000", "0x0000000000000000"),
    ("zeros_min", 5, 65, "0x8000000000000000", "0x3ff0000000000000"),
    ("zeros_min", 50, 66, "0x0000000000000000", "0x3ff0000000000000"),
    ("zeros_min", 1000, 67, "NAN", "NAN"),
    ("zeros_min", 300000, 68, "0x8000000000000000", "0x3ff0000000000000"),
    ("zeros_max", 1, 69, "0x0000000000000000", "0x0000000000000000"),
    ("zeros_max", 2, 70, "NAN", "NAN"),
    ("zeros_max", 5, 71, "0xbff0000000000000", "0x0000000000000000"),
    ("zeros_max", 50, 72, "0xbff0000000000000", "0x0000000000000000"),
    ("zeros_max", 1000, 73, "NAN", "NAN"),
    ("zeros_max", 300000, 74, "0xbff0000000000000", "0x0000000000000000"),
    ("special", 1, 75, "0xc0de6cf738692b48", "0xc0de6cf738692b48"),
    ("special", 2, 76, "0xbde49aed4ddb0fa0", "0xbd3020875ff17e00"),
    ("special", 5, 77, "0x3cbbfc3708fa69a0", "0x3ff0000000000000"),
    ("special", 50, 78, "0xc3823afb7b84a61d", "0x43895ef2b5b89446"),
    ("special", 1000, 79, "0xfff0000000000000", "0x7ff0000000000000"),
    ("special", 300000, 80, "0xfff0000000000000", "0x7ff0000000000000"),
    ("wide", 1, 81, "0xbd47180f6230ee92", "0xbd47180f6230ee92"),
    ("wide", 2, 82, "0xbc261bbb8419f024", "0x4274aef35510c35a"),
    ("wide", 5, 83, "0xc0c67ff15b5095a3", "0x3f3aaf0069d3f672"),
    ("wide", 50, 84, "0xc3511e873bb9941a", "0x43680fd02561f480"),
    ("wide", 1000, 85, "0xc3afb8d2e1b60152", "0x43aede163a17187d"),
    ("wide", 300000, 86, "0xc3aff60225ca4b70", "0x43afffd753d52640"),
];

const SORT_CASES: &[(&str, usize, u64, u64, u64)] = &[
    ("zeros_no_nan", 2, 87, 0xa2853c72a6a2cbbb, 0xa2853b9826a2cc51),
    ("zeros_no_nan", 7, 88, 0x85f2942e43f34208, 0xab81a861b5fc2979),
    ("zeros_no_nan", 100, 89, 0x8b8a30a413c6ee2c, 0x5a799926f6824fe0),
    ("zeros_no_nan", 5000, 90, 0x498ebd64660ac7ca, 0x88578a0630b9921c),
    ("zeros_no_nan", 100000, 91, 0xb69e034837684b64, 0x1b07a93ded7fa1ed),
    ("wide", 2, 92, 0x8399ef5540886033, 0x0c25b1774231d74c),
    ("wide", 7, 93, 0xae9665e5c9e0c869, 0xbf054e3c187a494a),
    ("wide", 100, 94, 0x62ad0fbe68561bff, 0xe87aa2411329bb33),
    ("wide", 5000, 95, 0x1730b7c2bd069e85, 0x2a36b04329a0d05d),
    ("wide", 100000, 96, 0x17a355eb8a05ca6d, 0xc25ee78e7db3bace),
    ("intfloat", 2, 97, 0x43ed048df6c67e91, 0x4383028df7e5074d),
    ("intfloat", 7, 98, 0x7475da0341d13275, 0x3a688a540063594e),
    ("intfloat", 100, 99, 0xac4ed6f702c03c22, 0xdb0dfab0efb35f21),
    ("intfloat", 5000, 100, 0x8ff12d19c352f4b1, 0xbf3eba4cde64f586),
    ("intfloat", 100000, 101, 0x9e124434297bedd5, 0xf413e96069daae60),
    ("uniform", 2, 102, 0x8575e47d42338af0, 0xf62a6f47cadd32f6),
    ("uniform", 7, 103, 0x11548f4bf67959b8, 0x74188024583837e0),
    ("uniform", 100, 104, 0x200e407a3b9a5245, 0xe98ff50fc3e0c818),
    ("uniform", 5000, 105, 0x129c35008f9220d4, 0x08775ab6047fd442),
    ("uniform", 100000, 106, 0xf94fb3b14873134b, 0x30021507f507d29d),
];

/// `(seed, mask of a <op> b, mask of b <op> a)`; bits: ==, <, <=, >, >=.
const LEX_CASES: &[(u64, u8, u8)] = &[
    (107, 24, 6),
    (108, 24, 6),
    (109, 24, 6),
    (110, 6, 24),
    (111, 6, 24),
    (112, 24, 6),
    (113, 24, 6),
    (114, 24, 6),
    (115, 6, 24),
    (116, 24, 6),
    (117, 24, 6),
    (118, 6, 24),
    (119, 6, 24),
    (120, 24, 6),
    (121, 24, 6),
    (122, 24, 6),
    (123, 24, 6),
    (124, 6, 24),
    (125, 21, 21),
    (126, 24, 6),
    (127, 24, 6),
    (128, 24, 6),
    (129, 6, 24),
    (130, 6, 24),
    (131, 6, 24),
    (132, 24, 6),
    (133, 24, 6),
    (134, 6, 24),
    (135, 24, 6),
    (136, 24, 6),
    (137, 24, 6),
    (138, 6, 24),
    (139, 6, 24),
    (140, 6, 24),
    (141, 6, 24),
    (142, 24, 6),
    (143, 24, 6),
    (144, 24, 6),
    (145, 24, 6),
    (146, 24, 6),
    (147, 24, 6),
    (148, 6, 24),
    (149, 6, 24),
    (150, 6, 24),
    (151, 6, 24),
    (152, 24, 6),
    (153, 24, 6),
    (154, 24, 6),
    (155, 6, 24),
    (156, 24, 6),
    (157, 6, 24),
    (158, 24, 6),
    (159, 24, 6),
    (160, 24, 6),
    (161, 0, 0),
    (162, 24, 6),
    (163, 6, 24),
    (164, 6, 24),
    (165, 24, 6),
    (166, 6, 24),
    (167, 6, 24),
    (168, 6, 24),
    (169, 24, 6),
    (170, 6, 24),
    (171, 24, 6),
    (172, 24, 6),
    (173, 6, 24),
    (174, 6, 24),
    (175, 24, 6),
    (176, 6, 24),
    (177, 24, 6),
    (178, 24, 6),
    (179, 6, 24),
    (180, 6, 24),
    (181, 24, 6),
    (182, 6, 24),
    (183, 24, 6),
    (184, 24, 6),
    (185, 6, 24),
    (186, 6, 24),
    (187, 6, 24),
    (188, 24, 6),
    (189, 6, 24),
    (190, 24, 6),
    (191, 24, 6),
    (192, 24, 6),
    (193, 24, 6),
    (194, 24, 6),
    (195, 6, 24),
    (196, 6, 24),
    (197, 24, 6),
    (198, 6, 24),
    (199, 24, 6),
    (200, 24, 6),
    (201, 24, 6),
    (202, 24, 6),
    (203, 6, 24),
    (204, 24, 6),
    (205, 24, 6),
    (206, 6, 24),
    (207, 24, 6),
    (208, 24, 6),
    (209, 6, 24),
    (210, 6, 24),
    (211, 6, 24),
    (212, 21, 21),
    (213, 24, 6),
    (214, 6, 24),
    (215, 24, 6),
    (216, 6, 24),
    (217, 21, 21),
    (218, 24, 6),
    (219, 21, 21),
    (220, 6, 24),
    (221, 6, 24),
    (222, 6, 24),
    (223, 24, 6),
    (224, 24, 6),
    (225, 24, 6),
    (226, 6, 24),
    (227, 21, 21),
    (228, 24, 6),
    (229, 0, 0),
    (230, 6, 24),
    (231, 24, 6),
    (232, 6, 24),
    (233, 24, 6),
    (234, 24, 6),
    (235, 6, 24),
    (236, 24, 6),
    (237, 21, 21),
    (238, 24, 6),
    (239, 24, 6),
    (240, 0, 0),
    (241, 6, 24),
    (242, 24, 6),
    (243, 24, 6),
    (244, 0, 0),
    (245, 24, 6),
    (246, 24, 6),
    (247, 24, 6),
    (248, 6, 24),
    (249, 6, 24),
    (250, 6, 24),
    (251, 6, 24),
    (252, 24, 6),
    (253, 24, 6),
    (254, 24, 6),
    (255, 24, 6),
    (256, 6, 24),
    (257, 24, 6),
    (258, 24, 6),
    (259, 24, 6),
    (260, 24, 6),
    (261, 21, 21),
    (262, 24, 6),
    (263, 6, 24),
    (264, 6, 24),
    (265, 6, 24),
    (266, 21, 21),
    (267, 6, 24),
    (268, 6, 24),
    (269, 6, 24),
    (270, 24, 6),
    (271, 6, 24),
    (272, 21, 21),
    (273, 24, 6),
    (274, 6, 24),
    (275, 6, 24),
    (276, 6, 24),
    (277, 24, 6),
    (278, 6, 24),
    (279, 24, 6),
    (280, 6, 24),
    (281, 6, 24),
    (282, 24, 6),
    (283, 6, 24),
    (284, 21, 21),
    (285, 24, 6),
    (286, 6, 24),
    (287, 6, 24),
    (288, 6, 24),
    (289, 6, 24),
    (290, 24, 6),
    (291, 24, 6),
    (292, 6, 24),
    (293, 0, 0),
    (294, 24, 6),
    (295, 24, 6),
    (296, 21, 21),
    (297, 6, 24),
    (298, 24, 6),
    (299, 24, 6),
    (300, 6, 24),
    (301, 6, 24),
    (302, 24, 6),
    (303, 24, 6),
    (304, 6, 24),
    (305, 6, 24),
    (306, 6, 24),
    (307, 6, 24),
    (308, 24, 6),
    (309, 24, 6),
    (310, 6, 24),
    (311, 6, 24),
    (312, 24, 6),
    (313, 24, 6),
    (314, 24, 6),
    (315, 21, 21),
    (316, 24, 6),
    (317, 6, 24),
    (318, 24, 6),
    (319, 24, 6),
    (320, 24, 6),
    (321, 24, 6),
    (322, 6, 24),
    (323, 6, 24),
    (324, 6, 24),
    (325, 24, 6),
    (326, 6, 24),
    (327, 24, 6),
    (328, 24, 6),
    (329, 24, 6),
    (330, 6, 24),
    (331, 24, 6),
    (332, 24, 6),
    (333, 24, 6),
    (334, 6, 24),
    (335, 6, 24),
    (336, 6, 24),
    (337, 24, 6),
    (338, 24, 6),
    (339, 24, 6),
    (340, 24, 6),
    (341, 24, 6),
    (342, 24, 6),
    (343, 24, 6),
    (344, 6, 24),
    (345, 24, 6),
    (346, 24, 6),
    (347, 6, 24),
    (348, 6, 24),
    (349, 24, 6),
    (350, 6, 24),
    (351, 21, 21),
    (352, 6, 24),
    (353, 6, 24),
    (354, 24, 6),
    (355, 24, 6),
    (356, 24, 6),
    (357, 24, 6),
    (358, 24, 6),
    (359, 6, 24),
    (360, 24, 6),
    (361, 6, 24),
    (362, 24, 6),
    (363, 24, 6),
    (364, 6, 24),
    (365, 6, 24),
    (366, 24, 6),
    (367, 21, 21),
    (368, 24, 6),
    (369, 24, 6),
    (370, 24, 6),
    (371, 6, 24),
    (372, 21, 21),
    (373, 6, 24),
    (374, 6, 24),
    (375, 6, 24),
    (376, 24, 6),
    (377, 24, 6),
    (378, 21, 21),
    (379, 6, 24),
    (380, 24, 6),
    (381, 0, 0),
    (382, 24, 6),
    (383, 24, 6),
    (384, 6, 24),
    (385, 24, 6),
    (386, 6, 24),
    (387, 6, 24),
    (388, 6, 24),
    (389, 6, 24),
    (390, 21, 21),
    (391, 6, 24),
    (392, 24, 6),
    (393, 24, 6),
    (394, 24, 6),
    (395, 24, 6),
    (396, 24, 6),
    (397, 24, 6),
    (398, 24, 6),
    (399, 6, 24),
    (400, 6, 24),
    (401, 24, 6),
    (402, 6, 24),
    (403, 24, 6),
    (404, 6, 24),
    (405, 6, 24),
    (406, 6, 24),
];
