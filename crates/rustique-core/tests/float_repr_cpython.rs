//! `float_repr` against CPython's own `repr(float)`.
//!
//! Values are rebuilt here from the same xorshift64* streams and fixed walks as
//! `float_repr_cpython_gen.py`, which folded CPython's `repr` of each value into one FNV-1a
//! 64 hash per category (1.6 M values; identical on CPython 3.14.9 and 3.15.1). Regenerate
//! the table below with that script; `--dump <kind>` lists CPython's reprs of a category.

use rustique_core::float_repr;

const MASK_52: u64 = (1 << 52) - 1;
const FNV_OFFSET: u64 = 0xCBF2_9CE4_8422_2325;
const FNV_PRIME: u64 = 0x100_0000_01B3;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
}

fn pow2(e: i64) -> f64 {
    f64::from_bits(((e + 1023) as u64) << 52)
}

fn ldexp1(e: i32) -> f64 {
    // 2**e for every e in -1074..=1023 (subnormal powers included).
    if e >= -1022 { pow2(i64::from(e)) } else { f64::from_bits(1u64 << (e + 1074)) }
}

fn neighbours(x: f64, out: &mut Vec<f64>) {
    let b = x.to_bits() as i64;
    for d in -2..=2 {
        let c = b + d;
        if (0..0x7FF0_0000_0000_0000).contains(&c) {
            out.push(f64::from_bits(c as u64));
        }
    }
}

fn values_of(kind: &str, seed: u64, n: usize) -> Vec<f64> {
    let mut r = Rng(seed);
    let mut v = Vec::with_capacity(n.max(1024));
    match kind {
        "bits" => {
            while v.len() < n {
                let x = f64::from_bits(r.next());
                if x.is_finite() {
                    v.push(x);
                }
            }
        }
        "subnormal" => {
            for _ in 0..n {
                let b = r.next();
                let x = f64::from_bits(b & MASK_52);
                v.push(if b >> 63 == 1 { -x } else { x });
            }
        }
        "pow2" => {
            let mut nb = Vec::new();
            for e in -1074..1024 {
                nb.clear();
                neighbours(ldexp1(e), &mut nb);
                for &y in &nb {
                    v.push(y);
                    v.push(-y);
                }
            }
        }
        "pow10" => {
            let mut nb = Vec::new();
            for e in -325..309 {
                nb.clear();
                neighbours(format!("1e{e}").parse().unwrap(), &mut nb);
                for &y in &nb {
                    v.push(y);
                    v.push(-y);
                }
            }
        }
        "dyadic" => {
            for _ in 0..n {
                let m = (r.next() >> 11) >> (r.next() % 53);
                let e = (r.next() % 160) as i64 - 80;
                let x = m as f64 * pow2(e);
                v.push(if r.next() & 1 == 1 { -x } else { x });
            }
        }
        "decimal" => {
            for _ in 0..n {
                let digits = 1 + r.next() % 17;
                let d = r.next() % 10u64.pow(digits as u32);
                let e = (r.next() % 660) as i64 - 340;
                v.push(format!("{d}e{e}").parse().unwrap());
            }
        }
        "unit" => {
            let pow10: Vec<f64> = (-30..=30).map(|j| format!("1e{j}").parse().unwrap()).collect();
            for _ in 0..n {
                let u = (r.next() >> 11) as f64 * 2f64.powi(-53);
                let j = (r.next() % 61) as usize;
                v.push(u * pow10[j]);
            }
        }
        "int" => {
            for _ in 0..n {
                let bits = 53 + r.next() % 12;
                v.push((r.next() >> (64 - bits)) as f64);
            }
        }
        "cutoff" => {
            for base in [1e16f64, 1e-4, 1e-5, 1e15, 1e17, 1e-3, 1e-6] {
                let b = base.to_bits() as i64;
                for d in -(n as i64)..=(n as i64) {
                    let x = f64::from_bits((b + d) as u64);
                    v.push(x);
                    v.push(-x);
                }
            }
        }
        _ => panic!("unknown kind {kind}"),
    }
    v
}

#[test]
fn matches_cpython_repr() {
    let mut failures = Vec::new();
    for &(kind, seed, n, count, want) in ORACLE {
        let values = values_of(kind, seed, n);
        assert_eq!(values.len(), count, "{kind}: value count");
        let mut h = FNV_OFFSET;
        for &x in &values {
            for &c in float_repr::repr(x).as_bytes().iter().chain(b"\n") {
                h = (h ^ u64::from(c)).wrapping_mul(FNV_PRIME);
            }
        }
        if h != want {
            failures.push(format!("{kind}: {h:#018x} != {want:#018x}"));
        }
    }
    assert!(failures.is_empty(), "mismatches (dump with the generator's --dump): {failures:?}");
}

// BEGIN GENERATED (crates/rustique-core/tests/float_repr_cpython_gen.py)
// CPython 3.15.1: (kind, seed, n, values, FNV-1a 64 of repr + newline)
const ORACLE: &[(&str, u64, usize, usize, u64)] = &[
    ("bits", 0x9e3779b97f4a7c15, 400000, 400000, 0x204adbf6334acae5),
    ("subnormal", 0xd1b54a32d192ed03, 50000, 50000, 0x3aa3c88da4892159),
    ("pow2", 0x0, 0, 20978, 0x7e4970afa41a2712),
    ("pow10", 0x0, 0, 6332, 0x1d82fbc8274345e1),
    ("dyadic", 0x94d049bb133111eb, 300000, 300000, 0xba9ad3f0d6a8f9e5),
    ("decimal", 0xbf58476d1ce4e5b9, 200000, 200000, 0x037120a257844e26),
    ("unit", 0x2545f4914f6cdd1d, 200000, 200000, 0xc36f1a60bd47c20f),
    ("int", 0x632be59bd9b4e019, 100000, 100000, 0x981ac6e90078abc5),
    ("cutoff", 0x0, 2000, 56014, 0xcc824dc6ed5ff08e),
];
