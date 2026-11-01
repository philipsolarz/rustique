//! Group `statistics`: `mean`, `variance`, `pvariance`, `stdev`, `pstdev` for a `list` or
//! `tuple` of `int` (incl. `bool`) and finite `float` values, as vectorcall kernels on the
//! module's function objects. See docs/targets/statistics.md.
//!
//! OWNER: utils workstream.
//!
//! Exactness. CPython sums the values as exact fractions (`_sum`, `_ss`) and rounds once.
//! For ints and finite floats every value is `±m·2^e` (`e >= -1074`), so the kernels add them
//! exactly into fixed-scale integers (`rustique_core::stats::FixedAcc`: `Σx` at `2^-1074`,
//! `Σx²` at `2^-2148`; scale 0 for int-only data), then do the final rational arithmetic
//! with Python ints: the result float is CPython's correctly rounded `int / int` of the same
//! rational value (what `float(Fraction)` computes), the int result of int-only data is
//! exact, and `stdev`/`pstdev` run `_float_sqrt_of_frac` step for step on the reduced
//! fraction (`math.gcd`, `math.isqrt`, shifts). NaN/inf, other types (Decimal, Fraction,
//! numpy scalars, subclasses), iterators, `xbar`/`mu`, too few points and overflow fall
//! back to CPython's original.

use std::sync::atomic::{AtomicPtr, Ordering};

use pyo3::ffi;
use pyo3::prelude::*;
use rustique_core::stats::{FixedAcc, SCALE_X, SCALE_XX, float_terms};

use crate::accel::{self, Prepare, Reason, Refusal, TargetSpec};
use crate::targets::utils_kit::{Guard, O, call_original, is_type, keep, key, nargs, pin_global_keys, pin_keys};

const R: Ordering = Ordering::Relaxed;

static GCD: AtomicPtr<ffi::PyObject> = AtomicPtr::new(std::ptr::null_mut());
static ISQRT: AtomicPtr<ffi::PyObject> = AtomicPtr::new(std::ptr::null_mut());
static S_BIT_LENGTH: AtomicPtr<ffi::PyObject> = AtomicPtr::new(std::ptr::null_mut());

#[inline(always)]
fn ld(p: &AtomicPtr<ffi::PyObject>) -> O {
    p.load(R)
}

/// The exact sums of a dataset.
struct Sums {
    n: usize,
    any_float: bool,
    sx: FixedAcc,
    sxx: Option<FixedAcc>,
}

/// One pass over a list/tuple of exact ints (fitting i64), bools and finite floats.
/// `None`: outside the subset (nothing observable happened).
unsafe fn scan(data: O, squares: bool) -> Option<Sums> {
    // SAFETY: exact list/tuple read under the list's critical section (GIL builds: no other
    // thread runs while we read).
    unsafe {
        let list = is_type(data, &raw mut ffi::PyList_Type);
        if !list && !is_type(data, &raw mut ffi::PyTuple_Type) {
            return None;
        }
        crate::targets::utils_kit::with_cs(data, || {
            let (n, items) = if list {
                (ffi::PyList_GET_SIZE(data) as usize, (*data.cast::<ffi::PyListObject>()).ob_item as *const O)
            } else {
                (
                    ffi::PyTuple_GET_SIZE(data) as usize,
                    (*data.cast::<ffi::PyTupleObject>()).ob_item.as_ptr() as *const O,
                )
            };
            let mut any_float = false;
            for i in 0..n {
                let x = *items.add(i);
                if is_type(x, &raw mut ffi::PyFloat_Type) {
                    if !ffi::PyFloat_AS_DOUBLE(x).is_finite() {
                        return None;
                    }
                    any_float = true;
                } else if !is_type(x, &raw mut ffi::PyLong_Type) && !is_type(x, &raw mut ffi::PyBool_Type) {
                    return None;
                }
            }
            let (s1, s2) = if any_float { (SCALE_X, SCALE_XX) } else { (0, 0) };
            let mut sx = FixedAcc::new(s1 + 1100);
            let mut sxx = squares.then(|| FixedAcc::new(s2 + 2200));
            for i in 0..n {
                let x = *items.add(i);
                if is_type(x, &raw mut ffi::PyFloat_Type) {
                    let ((neg, m, sh), (sq, sqsh)) = float_terms(ffi::PyFloat_AS_DOUBLE(x));
                    sx.add(neg, m, sh);
                    if let Some(a) = sxx.as_mut() {
                        a.add(false, sq, sqsh);
                    }
                } else {
                    let v = crate::targets::utils_kit::int_value(x)?;
                    let m = u128::from(v.unsigned_abs());
                    sx.add(v < 0, m, s1);
                    if let Some(a) = sxx.as_mut() {
                        a.add(false, m * m, s2);
                    }
                }
            }
            Some(Sums { n, any_float, sx, sxx })
        })
    }
}

/// A Python int from a [`FixedAcc`] (pos − neg). New reference or NULL.
unsafe fn acc_to_long(a: &FixedAcc) -> O {
    // SAFETY: plain constructors.
    unsafe {
        let (p, n) = a.to_le_bytes();
        let flags = ffi::Py_ASNATIVEBYTES_LITTLE_ENDIAN | ffi::Py_ASNATIVEBYTES_UNSIGNED_BUFFER;
        let pl = ffi::PyLong_FromUnsignedNativeBytes(p.as_ptr().cast(), p.len(), flags);
        let nl = ffi::PyLong_FromUnsignedNativeBytes(n.as_ptr().cast(), n.len(), flags);
        let r = if pl.is_null() || nl.is_null() { std::ptr::null_mut() } else { ffi::PyNumber_Subtract(pl, nl) };
        ffi::Py_XDECREF(pl);
        ffi::Py_XDECREF(nl);
        r
    }
}

/// Owned Python object references, released on drop.
struct Owned(Vec<O>);

impl Owned {
    fn new() -> Self {
        Owned(Vec::new())
    }
    /// Keep `o` (NULL → `None`).
    fn k(&mut self, o: O) -> Option<O> {
        if o.is_null() {
            return None;
        }
        self.0.push(o);
        Some(o)
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        for &o in &self.0 {
            // SAFETY: our references (kernels run attached).
            unsafe { ffi::Py_DECREF(o) };
        }
    }
}

unsafe fn int(v: i128) -> O {
    // SAFETY: plain constructor.
    unsafe { crate::targets::utils_kit::long_from_i128(v) }
}

/// `_convert(num/den, T)`: an int when T is int and the division is exact, else the
/// correctly rounded float. New reference; `None` (error cleared): fall back.
unsafe fn convert(o: &mut Owned, num: O, den: O, t_int: bool) -> Option<O> {
    // SAFETY: Python ints.
    unsafe {
        if t_int {
            let qr = o.k(ffi::PyNumber_Divmod(num, den))?;
            let q = ffi::PyTuple_GET_ITEM(qr, 0);
            let r = ffi::PyTuple_GET_ITEM(qr, 1);
            if ffi::PyObject_IsTrue(r) == 0 {
                return Some(ffi::Py_NewRef(q));
            }
        }
        let f = ffi::PyNumber_TrueDivide(num, den);
        if f.is_null() {
            ffi::PyErr_Clear(); // OverflowError: CPython raises it (fall back)
            return None;
        }
        Some(f)
    }
}

/// `_float_sqrt_of_frac(n, m)` of the reduced fraction `num/den` (both > 0 or num = 0).
unsafe fn float_sqrt_of_frac(o: &mut Owned, num: O, den: O) -> Option<O> {
    // SAFETY: Python ints; `math.gcd`/`math.isqrt` are C functions.
    unsafe {
        let g = o.k(ffi::PyObject_CallFunctionObjArgs(ld(&GCD), num, den, std::ptr::null_mut::<ffi::PyObject>()))?;
        let n = o.k(ffi::PyNumber_FloorDivide(num, g))?;
        let m = o.k(ffi::PyNumber_FloorDivide(den, g))?;
        let bits = |x: O| -> Option<i64> {
            // x.bit_length() (int's C method)
            let b = ffi::PyObject_CallMethodNoArgs(x, ld(&S_BIT_LENGTH));
            if b.is_null() {
                ffi::PyErr_Clear();
                return None;
            }
            let v = ffi::PyLong_AsLongLong(b);
            ffi::Py_DECREF(b);
            Some(v)
        };
        // q = (n.bit_length() - m.bit_length() - _sqrt_bit_width) // 2
        let q = (bits(n)? - bits(m)? - 109).div_euclid(2);
        // _integer_sqrt_of_frac_rto(n, m): a = isqrt(n // m); a | (a*a*m != n)
        let rto = |o: &mut Owned, n: O, m: O| -> Option<O> {
            let nm = o.k(ffi::PyNumber_FloorDivide(n, m))?;
            let a = o.k(ffi::PyObject_CallOneArg(ld(&ISQRT), nm))?;
            let aa = o.k(ffi::PyNumber_Multiply(a, a))?;
            let aam = o.k(ffi::PyNumber_Multiply(aa, m))?;
            let ne = ffi::PyObject_RichCompareBool(aam, n, ffi::Py_NE);
            if ne < 0 {
                ffi::PyErr_Clear();
                return None;
            }
            let bit = o.k(ffi::PyLong_FromLong(ne.into()))?;
            o.k(ffi::PyNumber_Or(a, bit))
        };
        let (numer, denom) = if q >= 0 {
            let two_q = o.k(int(2 * i128::from(q)))?;
            let qo = o.k(int(i128::from(q)))?;
            let mshift = o.k(ffi::PyNumber_Lshift(m, two_q))?;
            let r = rto(o, n, mshift)?;
            (o.k(ffi::PyNumber_Lshift(r, qo))?, o.k(int(1))?)
        } else {
            let two_q = o.k(int(-2 * i128::from(q)))?;
            let qo = o.k(int(-i128::from(q)))?;
            let nshift = o.k(ffi::PyNumber_Lshift(n, two_q))?;
            let one = o.k(int(1))?;
            (rto(o, nshift, m)?, o.k(ffi::PyNumber_Lshift(one, qo))?)
        };
        let f = ffi::PyNumber_TrueDivide(numer, denom);
        if f.is_null() {
            ffi::PyErr_Clear();
            return None;
        }
        Some(f)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Mean,
    Variance,
    PVariance,
    Stdev,
    PStdev,
}

/// The result for `data`, or `None` to fall back.
unsafe fn compute(data: O, kind: Kind) -> Option<O> {
    // SAFETY: attached thread.
    unsafe {
        let s = scan(data, kind != Kind::Mean)?;
        let n = s.n;
        let min = match kind {
            Kind::Mean | Kind::PVariance | Kind::PStdev => 1,
            Kind::Variance | Kind::Stdev => 2,
        };
        if n < min {
            return None; // StatisticsError
        }
        let mut o = Owned::new();
        let t_int = !s.any_float;
        let (s1, s2) = if s.any_float { (SCALE_X, SCALE_XX) } else { (0, 0) };
        let sx = o.k(acc_to_long(&s.sx))?;
        let n_obj = o.k(ffi::PyLong_FromSize_t(n))?;
        if kind == Kind::Mean {
            // total / n
            let sc = o.k(int(i128::from(s1)))?;
            let den = o.k(ffi::PyNumber_Lshift(n_obj, sc))?;
            return convert(&mut o, sx, den, t_int);
        }
        let sxx = o.k(acc_to_long(s.sxx.as_ref()?))?;
        // ssd = (n*sxx - sx*sx) / n  (scale 2^s2), then / (n-1) or / n
        let nsxx = o.k(ffi::PyNumber_Multiply(n_obj, sxx))?;
        let sx2 = o.k(ffi::PyNumber_Multiply(sx, sx))?;
        let num = o.k(ffi::PyNumber_Subtract(nsxx, sx2))?;
        let d = match kind {
            Kind::Variance | Kind::Stdev => n as i128 * (n as i128 - 1),
            _ => n as i128 * n as i128,
        };
        let d_obj = o.k(int(d))?;
        let sc = o.k(int(i128::from(s2)))?;
        let den = o.k(ffi::PyNumber_Lshift(d_obj, sc))?;
        match kind {
            Kind::Variance | Kind::PVariance => convert(&mut o, num, den, t_int),
            _ => float_sqrt_of_frac(&mut o, num, den),
        }
    }
}

static G_MEAN: Guard = Guard::new(&STATS_DEPS);
static G_VARIANCE: Guard = Guard::new(&STATS_DEPS);
static G_PVARIANCE: Guard = Guard::new(&STATS_DEPS);
static G_STDEV: Guard = Guard::new(&STATS_DEPS);
static G_PSTDEV: Guard = Guard::new(&STATS_DEPS);

unsafe fn kernel(g: &Guard, spec: &'static TargetSpec, kind: Kind, func: O, args: *const O, nargsf: usize, kw: O) -> O {
    // SAFETY: vectorcall arguments.
    unsafe {
        // (data) or (data, None): xbar/mu given → CPython's other code path.
        let n = nargs(nargsf);
        let ok_shape = kw.is_null() && (n == 1 || (n == 2 && *args.add(1) == ffi::Py_None()));
        if ok_shape {
            if !g.ready(func) {
                return call_original(func, args, nargsf, kw);
            }
            if let Some(r) = compute(*args, kind) {
                if g.counters.hit_check_streak() {
                    let argv: Vec<O> = (0..n).map(|i| *args.add(i)).collect();
                    return accel::check::verify(spec, r, &argv, || call_original(func, args, nargsf, kw));
                }
                return r;
            }
        }
        g.counters.fallback_streak(Reason::Args);
        call_original(func, args, nargsf, kw)
    }
}

unsafe extern "C" fn vc_mean(func: O, args: *const O, nargsf: usize, kw: O) -> O {
    // SAFETY: vectorcall arguments.
    unsafe { kernel(&G_MEAN, &MEAN, Kind::Mean, func, args, nargsf, kw) }
}
unsafe extern "C" fn vc_variance(func: O, args: *const O, nargsf: usize, kw: O) -> O {
    // SAFETY: vectorcall arguments.
    unsafe { kernel(&G_VARIANCE, &VARIANCE, Kind::Variance, func, args, nargsf, kw) }
}
unsafe extern "C" fn vc_pvariance(func: O, args: *const O, nargsf: usize, kw: O) -> O {
    // SAFETY: vectorcall arguments.
    unsafe { kernel(&G_PVARIANCE, &PVARIANCE, Kind::PVariance, func, args, nargsf, kw) }
}
unsafe extern "C" fn vc_stdev(func: O, args: *const O, nargsf: usize, kw: O) -> O {
    // SAFETY: vectorcall arguments.
    unsafe { kernel(&G_STDEV, &STDEV, Kind::Stdev, func, args, nargsf, kw) }
}
unsafe extern "C" fn vc_pstdev(func: O, args: *const O, nargsf: usize, kw: O) -> O {
    // SAFETY: vectorcall arguments.
    unsafe { kernel(&G_PSTDEV, &PSTDEV, Kind::PStdev, func, args, nargsf, kw) }
}

fn prepare_common(cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    if S_BIT_LENGTH.load(R).is_null() {
        S_BIT_LENGTH.store(crate::targets::utils_kit::interned(cx.py(), "bit_length"), R);
    }
    let math = cx.module_attr("math")?.cast_into::<pyo3::types::PyModule>().map_err(PyErr::from)?;
    let d = math.dict();
    keep(&GCD, &cx.get(&d, "gcd")?);
    keep(&ISQRT, &cx.get(&d, "isqrt")?);
    if cx.module_attr("_sqrt_bit_width")?.extract::<i64>()? != 109 {
        return Err(Refusal::new("_sqrt_bit_width differs"));
    }
    Ok(())
}

/// Pins of the group (`SHARED`): the module globals, `math.gcd`/`math.isqrt` and the
/// builtins the five originals read on the accelerated paths (error-only names are not
/// pinned: on the inputs the kernels accept, CPython never reads them).
static STATS_DEPS: accel::DepFlag = accel::DepFlag::new();

/// The group's shared declarations (prepared once before the first target).
fn prepare_group<'py>(_py: Python<'py>, cx: &mut Prepare<'py>) -> Result<(), Refusal> {
    prepare_common(cx)?;
    let globals = cx.module()?.dict();
    pin_keys!(
        cx,
        &globals,
        &STATS_DEPS,
        [
            "_sum",
            "_ss",
            "_convert",
            "_exact_ratio",
            "_coerce",
            "Fraction",
            "Decimal",
            "groupby",
            "reduce",
            "math",
            "_float_sqrt_of_frac",
            "_integer_sqrt_of_frac_rto",
            "_sqrt_bit_width",
        ]
    );
    let math = cx.module_attr("math")?.cast_into::<pyo3::types::PyModule>().map_err(PyErr::from)?;
    pin_keys!(cx, &math.dict(), &STATS_DEPS, ["gcd", "isqrt"]);
    pin_global_keys!(cx, &globals, &STATS_DEPS, ["map", "sum", "set", "type", "issubclass"]);
    Ok(())
}

/// Shared declarations of the five targets.
pub static SHARED: accel::GroupSpec = accel::GroupSpec { name: "statistics", prepare: prepare_group };

/// A target's own declarations: fingerprint, guard identity, site.
fn prepare_fn<'py>(
    cx: &mut Prepare<'py>,
    attr: &Bound<'py, pyo3::types::PyString>,
    fp: accel::Fingerprints,
    g: &'static Guard,
    kernel: ffi::vectorcallfunc,
) -> Result<(), Refusal> {
    let func =
        cx.get_opt_s(&cx.module()?.dict(), attr)?.ok_or_else(|| Refusal::new(format!("statistics.{attr} missing")))?;
    cx.fingerprint_code(&func, fp)?;
    g.capture(&func);
    cx.vectorcall(&func, kernel, &accel::FUNCTION_VECTORCALL)
}

macro_rules! target {
    ($spec:ident, $prep:ident, $name:literal, $attr:literal, $kernel:ident, $guard:ident, $fp:expr, $_sqrt:expr) => {
        fn $prep<'py>(py: Python<'py>, cx: &mut Prepare<'py>) -> Result<(), Refusal> {
            prepare_fn(cx, &key(py, $attr), $fp, &$guard, $kernel)
        }
        pub static $spec: TargetSpec = TargetSpec {
            name: $name,
            group: "statistics",
            module: Some("statistics"),
            versions: &[(3, 14), (3, 15)],
            free_threaded: true,
            counters: &$guard.counters,
            prepare: $prep,
            checkable: true,
            shared: Some(&SHARED),
            ..TargetSpec::DEFAULT
        };
    };
}

target!(
    MEAN,
    prep_mean,
    "statistics.mean",
    "mean",
    vc_mean,
    G_MEAN,
    &[(3, 14, "aede1e231da734c8"), (3, 15, "6ea74ea49665b2f8")],
    false
);
target!(
    VARIANCE,
    prep_variance,
    "statistics.variance",
    "variance",
    vc_variance,
    G_VARIANCE,
    &[(3, 14, "69a1cb731cbacbfb"), (3, 15, "75d05a4f59205ba3")],
    false
);
target!(
    PVARIANCE,
    prep_pvariance,
    "statistics.pvariance",
    "pvariance",
    vc_pvariance,
    G_PVARIANCE,
    &[(3, 14, "a49fa71b6f4e3479"), (3, 15, "4920b5933f38b272")],
    false
);
target!(
    STDEV,
    prep_stdev,
    "statistics.stdev",
    "stdev",
    vc_stdev,
    G_STDEV,
    &[(3, 14, "c9cffbff8f4a75b9"), (3, 15, "ab6b25030f5fadbc")],
    true
);
target!(
    PSTDEV,
    prep_pstdev,
    "statistics.pstdev",
    "pstdev",
    vc_pstdev,
    G_PSTDEV,
    &[(3, 14, "e7164b1668097211"), (3, 15, "7c2a7d7cebcdf53e")],
    true
);

pub static TARGETS: &[&TargetSpec] = &[&MEAN, &VARIANCE, &PVARIANCE, &STDEV, &PSTDEV];
