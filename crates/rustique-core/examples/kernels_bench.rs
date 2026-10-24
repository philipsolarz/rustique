//! Timing harness for the column kernels (no criterion; fresh inputs, verified results).
//!
//! ```text
//! cargo run --release -p rustique-core --example kernels_bench            # kernel timings
//! cargo run --release -p rustique-core --example kernels_bench -- tune    # rayon crossovers
//! ```

use std::hint::black_box;
use std::time::{Duration, Instant};

use rayon::prelude::*;
use rustique_core::kernels::{self, Sum};
use rustique_core::{Column, Value};

/// xorshift64*: deterministic, dependency-free test data.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn f64(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64 * 2e6 - 1e6
    }
}

fn i64s(n: usize, seed: u64) -> Vec<i64> {
    let mut r = Rng(seed);
    (0..n).map(|_| r.next() as i64 >> 1).collect()
}

fn f64s(n: usize, seed: u64) -> Vec<f64> {
    let mut r = Rng(seed);
    (0..n).map(|_| r.f64()).collect()
}

/// Best of `reps` runs of `f` on a fresh input from `make`.
fn time<I, R>(reps: usize, mut make: impl FnMut() -> I, mut f: impl FnMut(I) -> R) -> Duration {
    (0..reps)
        .map(|_| {
            let input = make();
            let t = Instant::now();
            black_box(f(black_box(input)));
            t.elapsed()
        })
        .min()
        .unwrap()
}

fn report(name: &str, d: Duration) {
    println!("{name:<44} {:>10.3} ms", d.as_secs_f64() * 1e3);
}

fn kernels_suite() {
    const N: usize = 10_000_000;
    const M: usize = 1_000_000;
    let ints = Column::I64(i64s(N, 1));
    let floats = Column::F64(f64s(N, 2));
    let expect: i128 = ints.as_slice::<i64>().unwrap().iter().map(|&x| x as i128).sum();
    assert_eq!(kernels::sum(&ints).unwrap(), Sum::Int(expect));

    report("sum i64 x10M", time(7, || &ints, kernels::sum));
    report(
        "sum i64 x10M (naive i128 loop)",
        time(7, || &ints, |c| c.as_slice::<i64>().unwrap().iter().map(|&x| x as i128).sum::<i128>()),
    );
    report("sum f64 x10M (CPython-exact Neumaier)", time(7, || &floats, kernels::sum));
    report(
        "sum f64 x10M (plain fold, reference)",
        time(7, || &floats, |c| c.as_slice::<f64>().unwrap().iter().sum::<f64>()),
    );
    let i32s = Column::I32(i64s(N, 3).into_iter().map(|x| x as i32).collect());
    report("sum i32 x10M", time(7, || &i32s, kernels::sum));
    let bools = Column::Bool(i64s(N, 3).into_iter().map(|x| x & 1 == 1).collect());
    report("sum bool x10M", time(7, || &bools, kernels::sum));

    report("min i64 x10M", time(7, || &ints, kernels::min));
    report("max f64 x10M", time(7, || &floats, kernels::max));
    report(
        "max f64 x10M (sequential CPython loop)",
        time(
            7,
            || &floats,
            |c| {
                let v = c.as_slice::<f64>().unwrap();
                v.iter().copied().fold(v[0], |m, x| if x > m { x } else { m })
            },
        ),
    );
    report("argmax f64 x10M", time(7, || &floats, kernels::argmax));

    let absent = Value::I64(-1);
    report("find i64 x10M (absent)", time(7, || &ints, |c| kernels::find(c, &absent, 0, N)));
    report(
        "find i64 x10M (absent, Iterator::position)",
        time(7, || &ints, |c| c.as_slice::<i64>().unwrap().iter().position(|&x| x == -1)),
    );
    report("count i64 x10M", time(7, || &ints, |c| kernels::count(c, &absent)));
    report("find f64 x10M (absent int needle)", time(7, || &floats, |c| kernels::find(c, &Value::I64(3), 0, N)));
    let ints2 = ints.clone();
    report("equal i64 x10M (equal)", time(7, || (&ints, &ints2), |(a, b)| kernels::equal(a, b)));
    let as_f64 = Column::F64(ints.as_slice::<i64>().unwrap().iter().map(|&x| (x >> 12) as f64).collect());
    let as_i64 = Column::I64(ints.as_slice::<i64>().unwrap().iter().map(|&x| x >> 12).collect());
    report("equal i64 vs f64 x10M (equal)", time(7, || (&as_i64, &as_f64), |(a, b)| kernels::equal(a, b)));

    let base_f = f64s(M, 4);
    let base_i = i64s(M, 5);
    let mut sorted = Column::F64(base_f.clone());
    kernels::sort(&mut sorted, false);
    let mut check = base_f.clone();
    check.sort_by(f64::total_cmp);
    assert_eq!(sorted.as_slice::<f64>().unwrap(), &check[..]);
    report("sort f64 x1M", time(7, || Column::F64(base_f.clone()), |mut c| kernels::sort(&mut c, false)));
    report("sort f64 x1M reverse", time(7, || Column::F64(base_f.clone()), |mut c| kernels::sort(&mut c, true)));
    report(
        "sort f64 x1M (std stable sort_by, 1 thread)",
        time(
            7,
            || base_f.clone(),
            |mut v| {
                v.sort_by(|a, b| a.partial_cmp(b).unwrap());
                v
            },
        ),
    );
    report(
        "sort f64 x1M (std unstable, 1 thread)",
        time(
            7,
            || base_f.clone(),
            |mut v| {
                v.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap());
                v
            },
        ),
    );
    report(
        "sort f64 x1M (rayon stable par_sort_by)",
        time(
            7,
            || base_f.clone(),
            |mut v| {
                v.par_sort_by(|a, b| a.partial_cmp(b).unwrap());
                v
            },
        ),
    );
    let mut with_nan = base_f.clone();
    for x in with_nan.iter_mut().step_by(10) {
        *x = f64::NAN;
    }
    report("sort f64 x1M (10% NaN)", time(7, || Column::F64(with_nan.clone()), |mut c| kernels::sort(&mut c, false)));
    report("sort i64 x1M", time(7, || Column::I64(base_i.clone()), |mut c| kernels::sort(&mut c, false)));
    let small: Vec<i16> = base_i.iter().map(|&x| x as i16).collect();
    report("sort i16 x1M (counting)", time(7, || Column::I16(small.clone()), |mut c| kernels::sort(&mut c, false)));
    let f64col = Column::F64(base_f.clone());
    report("argsort f64 x1M", time(7, || &f64col, |c| kernels::argsort(c, false)));
    let base_f32: Vec<f32> = base_f.iter().map(|&x| x as f32).collect();
    let f32col = Column::F32(base_f32);
    report("argsort f32 x1M (packed u64)", time(7, || &f32col, |c| kernels::argsort(c, false)));
    let i64col = Column::I64(base_i.clone());
    report("argsort i64 x1M", time(7, || &i64col, |c| kernels::argsort(c, false)));
    let strs: Vec<String> = base_i.iter().take(M).map(|x| format!("s{x:x}")).collect();
    let strs = Column::from(strs.iter().map(String::as_str).collect::<Vec<_>>());
    report("sort str x1M", time(5, || strs.clone(), |mut c| kernels::sort(&mut c, false)));
    report("argsort str x1M", time(5, || &strs, |c| kernels::argsort(c, false)));
}

/// Sequential vs rayon timings around candidate thresholds.
fn tune() {
    for shift in [10, 12, 13, 14, 15, 16, 17, 18, 20] {
        let n = 1usize << shift;
        let reps = (1 << 24 >> shift).clamp(5, 2000);
        let f = f64s(n, 9);
        let seq = time(reps, || f.clone(), |mut v| v.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap()));
        let par = time(reps, || f.clone(), |mut v| v.par_sort_unstable_by(|a, b| a.partial_cmp(b).unwrap()));
        let i = i64s(n, 9);
        let seq_sum =
            time(reps, || &i, |v| v.chunks(1 << 16).map(|c| c.iter().map(|&x| x as i128).sum::<i128>()).sum::<i128>());
        let par_sum = time(
            reps,
            || &i,
            |v| v.par_chunks(1 << 16).map(|c| c.iter().map(|&x| x as i128).sum::<i128>()).sum::<i128>(),
        );
        let seq_cnt = time(reps, || &i, |v| v.iter().filter(|&&x| x == 7).count());
        let par_cnt =
            time(reps, || &i, |v| v.par_chunks(1 << 16).map(|c| c.iter().filter(|&&x| x == 7).count()).sum::<usize>());
        let s16: Vec<i16> = i.iter().map(|&x| x as i16).collect();
        let seq16 = time(reps, || s16.clone(), |mut v| v.sort_unstable());
        let cnt16 = time(reps, || Column::I16(s16.clone()), |mut c| kernels::sort(&mut c, false));
        println!(
            "n=2^{shift:<2} sort seq {:>9.1}us par {:>9.1}us | sum seq {:>8.1}us par {:>8.1}us | count seq {:>8.1}us par {:>8.1}us | i16 pdq {:>8.1}us kernel {:>8.1}us",
            us(seq),
            us(par),
            us(seq_sum),
            us(par_sum),
            us(seq_cnt),
            us(par_cnt),
            us(seq16),
            us(cnt16)
        );
    }
}

fn us(d: Duration) -> f64 {
    d.as_secs_f64() * 1e6
}

fn main() {
    println!("rayon threads: {}", rayon::current_num_threads());
    if std::env::args().any(|a| a == "tune") { tune() } else { kernels_suite() }
}
