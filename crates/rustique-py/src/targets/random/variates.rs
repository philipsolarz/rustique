//! The continuous distributions of `random.Random` that custom simulations call in hot
//! loops — `normalvariate`, `lognormvariate`, `expovariate`, `paretovariate`, `gauss` —
//! as vectorcall kernels on the function objects in `Random.__dict__` (docs/targets/
//! random.md §7). They share the group's machinery (`self.random` resolved without running
//! Python code, the native generator on the object's own state) and add:
//!
//! - **the same stream, consumed in the same amounts**: each kernel draws exactly the
//!   `random()` values the method draws (`normalvariate`'s rejection loop two per round,
//!   `gauss` two when `gauss_next` is `None` and none otherwise), from the same state;
//! - **the same floats, bit for bit**: the same IEEE operations in the same order
//!   (`rustique_core::random`), the C library functions `random.py` binds from `math`
//!   (`_log`, `_exp`, `_sqrt`, `_cos`, `_sin`) and `float_pow` calls (`pow`), checked
//!   against `math` on probe values before the first use (a mismatch degrades the five
//!   targets), and the module's constants (`NV_MAGICCONST`, `TWOPI`) read from the pinned
//!   module globals;
//! - **no error after the stream moved**: every input for which the method would raise
//!   (`expovariate(0)`, `paretovariate(0)`: `ZeroDivisionError` after one `random()`;
//!   `lognormvariate`'s `exp` overflow; `paretovariate`'s `pow` range errors) is
//!   recognised before anything observable and runs CPython's original, which consumes
//!   and raises. The kernels accept `mu`/`sigma`/`lambd`/`alpha` that are exact `float`s or
//!   exact `int`s within ±2**63 (converted as `float` arithmetic converts them, correctly
//!   rounded), with `lognormvariate` limited to `mu + 12.5 |sigma| <= 709` (an accepted
//!   normal deviate has `|z| < 12.2`, so `exp` cannot overflow) and `paretovariate` to
//!   `1/16 <= |alpha|` (no `pow` range error).
//!
//! `lognormvariate` calls `self.normalvariate(mu, sigma)`: the kernel requires that
//! attribute to resolve to CPython's `Random.normalvariate` (class and instance, as
//! `LOAD_ATTR` does), with its validated code and no tool listening to it, and then runs
//! the same loop. `gauss` reads and writes the instance attribute `gauss_next` (part of
//! `getstate()`): only for instances whose attributes live in their inline values, with
//! the generic `__setattr__` and no `gauss_next` descriptor in the class, through
//! `PyObject_GenericGetAttr`/`PyObject_GenericSetAttr` (what `LOAD_ATTR`/`STORE_ATTR`
//! do there), in CPython's order: read, store `None`, draw, store the second deviate.

use std::sync::atomic::{AtomicPtr, AtomicU8, AtomicU64};

use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::PyType;

use super::{MT_BAD, R, RANDOM_CLS, S, bind, ld, mt, prepare_statics, prepare_target, random_native};
use crate::accel::{self, Prepare, Reason, Refusal, TargetSpec};
use crate::targets::utils_kit::{
    _PyType_Lookup, Guard, INST, O, call_original, generic_getattro, get_s, interned, is_type, keep, key,
    module_dict_s, nargs, prepare_symbols, with_cs,
};
use rustique_core::random::{NORMAL_Z_BOUND, libm, variates as cv};

/// Interned names of the variates (process lifetime).
struct Names {
    normalvariate: AtomicPtr<ffi::PyObject>,
    gauss_next: AtomicPtr<ffi::PyObject>,
    mu: AtomicPtr<ffi::PyObject>,
    sigma: AtomicPtr<ffi::PyObject>,
    lambd: AtomicPtr<ffi::PyObject>,
    alpha: AtomicPtr<ffi::PyObject>,
}

macro_rules! null {
    () => {
        AtomicPtr::new(std::ptr::null_mut())
    };
}

static V: Names =
    Names { normalvariate: null!(), gauss_next: null!(), mu: null!(), sigma: null!(), lambd: null!(), alpha: null!() };

/// `random.NV_MAGICCONST` and `random.TWOPI` (pinned; their values, as bits).
static NV_MAGICCONST: AtomicU64 = AtomicU64::new(0);
static TWOPI: AtomicU64 = AtomicU64::new(0);

/// `Random.normalvariate` and its validated code object (for `lognormvariate`).
static NORMAL_FN: AtomicPtr<ffi::PyObject> = null!();
static NORMAL_CODE: AtomicPtr<ffi::PyObject> = null!();

/// `random._log`, `_exp`, `_cos`, `_sin`, `_sqrt` (= `math.log`, ...): the probe self-test's
/// reference.
static LOG_FN: AtomicPtr<ffi::PyObject> = null!();
static EXP_FN: AtomicPtr<ffi::PyObject> = null!();
static COS_FN: AtomicPtr<ffi::PyObject> = null!();
static SIN_FN: AtomicPtr<ffi::PyObject> = null!();
static SQRT_FN: AtomicPtr<ffi::PyObject> = null!();

/// Pins of the variates (`VARIATES`): the module globals they read.
static VARIATE_DEPS: accel::DepFlag = accel::DepFlag::new();

static G_NORMAL: Guard = Guard::new(&VARIATE_DEPS);
static G_LOGNORM: Guard = Guard::new(&VARIATE_DEPS);
static G_EXPO: Guard = Guard::new(&VARIATE_DEPS);
static G_PARETO: Guard = Guard::new(&VARIATE_DEPS);
static G_GAUSS: Guard = Guard::new(&VARIATE_DEPS);

// ---------------------------------------------------------------------------------------
// The C library functions: checked against `math` before the first use.
// ---------------------------------------------------------------------------------------

static LIBM_OK: AtomicU8 = AtomicU8::new(LIBM_UNKNOWN);
const LIBM_UNKNOWN: u8 = 0;
const LIBM_GOOD: u8 = 1;
const LIBM_BAD: u8 = 2;
const LIBM_FAILED: &str = "random: C library log/exp/cos/sin/pow differ from math's on probe values";

#[inline(always)]
fn libm_ready() -> bool {
    match LIBM_OK.load(R) {
        LIBM_GOOD => true,
        LIBM_UNKNOWN => libm_check(),
        _ => false,
    }
}

/// Compare [`libm`] with `math.log/exp/cos/sin/sqrt` and `float ** float` on probe values
/// spanning the arguments the variates pass (C calls only, private objects).
#[cold]
#[inline(never)]
fn libm_check() -> bool {
    // SAFETY: kernels run attached; the statics hold strong references to C functions.
    let ok = unsafe { libm_probe() };
    let new = if ok { LIBM_GOOD } else { LIBM_BAD };
    if LIBM_OK.compare_exchange(LIBM_UNKNOWN, new, R, R).is_ok() && !ok {
        for spec in TARGETS {
            accel::degrade(spec, LIBM_FAILED);
        }
    }
    ok
}

/// `f(x)` through a C function object; `None` on any error (cleared).
unsafe fn call1(f: O, x: f64) -> Option<f64> {
    // SAFETY: attached thread; `f` a C function taking a float.
    unsafe {
        let a = ffi::PyFloat_FromDouble(x);
        if a.is_null() {
            ffi::PyErr_Clear();
            return None;
        }
        let r = ffi::PyObject_CallOneArg(f, a);
        ffi::Py_DECREF(a);
        if r.is_null() || !is_type(r, &raw mut ffi::PyFloat_Type) {
            ffi::PyErr_Clear();
            ffi::Py_XDECREF(r);
            return None;
        }
        let v = ffi::PyFloat_AS_DOUBLE(r);
        ffi::Py_DECREF(r);
        Some(v)
    }
}

/// `x ** y` for floats (`float_pow`); `None` on error.
unsafe fn pow2(x: f64, y: f64) -> Option<f64> {
    // SAFETY: attached thread; float operands.
    unsafe {
        let a = ffi::PyFloat_FromDouble(x);
        let b = ffi::PyFloat_FromDouble(y);
        let r =
            if a.is_null() || b.is_null() { std::ptr::null_mut() } else { ffi::PyNumber_Power(a, b, ffi::Py_None()) };
        ffi::Py_XDECREF(a);
        ffi::Py_XDECREF(b);
        if r.is_null() || !is_type(r, &raw mut ffi::PyFloat_Type) {
            ffi::PyErr_Clear();
            ffi::Py_XDECREF(r);
            return None;
        }
        let v = ffi::PyFloat_AS_DOUBLE(r);
        ffi::Py_DECREF(r);
        Some(v)
    }
}

unsafe fn libm_probe() -> bool {
    // SAFETY: per `libm_check`.
    unsafe {
        let same = |a: Option<f64>, b: f64| a.is_some_and(|a| a.to_bits() == b.to_bits());
        // Deterministic probes: 48 values of `1.0 - random()` from a 53-bit LCG, plus the
        // ends of the range.
        let mut s: u64 = 0x9e37_79b9_7f4a_7c15;
        let mut us = [0.0f64; 56];
        us[..8].copy_from_slice(&[1.0, 0.5, 2f64.powi(-53), 1.0 - 2f64.powi(-53), 0.1, 0.25, 0.75, 0.9]);
        for u in us.iter_mut().skip(8) {
            s = s.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            *u = 1.0 - ((s >> 11) as f64) * (1.0 / 9_007_199_254_740_992.0);
        }
        let (log_f, exp_f, cos_f, sin_f, sqrt_f) = (ld(&LOG_FN), ld(&EXP_FN), ld(&COS_FN), ld(&SIN_FN), ld(&SQRT_FN));
        let twopi = f64::from_bits(TWOPI.load(R));
        for (i, &u) in us.iter().enumerate() {
            let l = libm::log(u);
            let x = (1.0 - u) * twopi;
            let e = -700.0 + 25.0 * (i as f64) + u; // exp's whole finite range
            let p = [-0.5, -1.0, -1.0 / 3.0, -2.0, -16.0, 0.25, 16.0][i % 7];
            if !same(call1(log_f, u), l)
                || !same(call1(sqrt_f, -2.0 * l), libm::sqrt(-2.0 * l))
                || !same(call1(cos_f, x), libm::cos(x))
                || !same(call1(sin_f, x), libm::sin(x))
                || !same(call1(exp_f, e), libm::exp(e))
                || !same(pow2(u, p), if u == 1.0 { 1.0 } else { libm::pow(u, p) })
            {
                return false;
            }
        }
        true
    }
}

// ---------------------------------------------------------------------------------------
// Operands
// ---------------------------------------------------------------------------------------

/// A parameter as `float` arithmetic reads it (`CONVERT_TO_DOUBLE`): an exact `float`, or
/// an exact `int` within ±2**63 (converted with correct rounding, as `PyLong_AsDouble`;
/// Rust's `i64 as f64` rounds to nearest-even too). Anything else (`bool`, subclasses
/// with their own operators, `Decimal`, `Fraction`, numpy scalars, big ints): `None`.
#[inline(always)]
unsafe fn operand(o: O) -> Option<f64> {
    // SAFETY: live object or NULL.
    unsafe {
        if o.is_null() {
            return None;
        }
        if is_type(o, &raw mut ffi::PyFloat_Type) {
            return Some(ffi::PyFloat_AS_DOUBLE(o));
        }
        if is_type(o, &raw mut ffi::PyLong_Type) {
            let mut overflow = 0;
            let v = ffi::PyLong_AsLongLongAndOverflow(o, &mut overflow);
            if overflow == 0 && !(v == -1 && !ffi::PyErr_Occurred().is_null()) {
                return Some(v as f64);
            }
            ffi::PyErr_Clear();
        }
        None
    }
}

/// The argument, or (NULL) `func.__defaults__[i]` (the guard checks the defaults are the
/// validated tuple before the kernel acts; NULL if absent).
#[inline(always)]
unsafe fn or_default(func: O, arg: O, i: isize) -> O {
    // SAFETY: `func` is a function object.
    unsafe {
        if !arg.is_null() {
            return arg;
        }
        let d = (*func.cast::<ffi::PyFunctionObject>()).func_defaults;
        if d.is_null() || ffi::PyTuple_GET_SIZE(d) <= i {
            return std::ptr::null_mut();
        }
        ffi::PyTuple_GET_ITEM(d, i)
    }
}

/// A new float (NULL with MemoryError: CPython would fail at the same point).
#[inline(always)]
unsafe fn float(x: f64) -> O {
    // SAFETY: attached thread.
    unsafe { ffi::PyFloat_FromDouble(x) }
}

/// Native generator for `self.random()` and the C library functions verified.
#[inline(always)]
unsafe fn native(slf: O) -> bool {
    // SAFETY: attached thread.
    unsafe { random_native(slf) && libm_ready() }
}

// ---------------------------------------------------------------------------------------
// Kernels
// ---------------------------------------------------------------------------------------

/// `normalvariate`'s loop: each round draws its two `random()` values under the
/// generator's critical section (CPython takes it once per `random()` call, so another
/// thread may draw between rounds there too) and tests acceptance outside it.
#[inline]
unsafe fn normal_z(slf: O, nv: f64) -> f64 {
    // SAFETY: attached thread; `slf` checked by `random_native`.
    unsafe {
        loop {
            let (u1, r) = with_cs(slf, || mt(slf).random2());
            if let Some(z) = cv::normal_try(nv, u1, r) {
                return z;
            }
        }
    }
}

/// `Random.expovariate(self, lambd=1.0)`: `-_log(1.0 - self.random()) / lambd`.
unsafe extern "C" fn vc_expovariate(func: O, args: *const O, nargsf: usize, kw: O) -> O {
    // SAFETY: vectorcall arguments.
    unsafe {
        let g = &G_EXPO;
        if let Some([slf, l]) = bind(args, nargs(nargsf), kw, [std::ptr::null_mut(), ld(&V.lambd)])
            && !slf.is_null()
            && let Some(lv) = operand(or_default(func, l, 0))
            && lv != 0.0
        {
            if !g.ready(func) {
                return call_original(func, args, nargsf, kw);
            }
            if native(slf) {
                // The draw under the generator's lock; the arithmetic outside it (shorter
                // critical sections when threads share a generator).
                let r = with_cs(slf, || mt(slf).random());
                g.counters.hit_streak();
                return float(cv::expo(r) / lv);
            }
        }
        g.counters.fallback_streak(Reason::Args);
        call_original(func, args, nargsf, kw)
    }
}

/// `Random.paretovariate(self, alpha)`: `(1.0 - self.random()) ** (-1.0 / alpha)`.
unsafe extern "C" fn vc_paretovariate(func: O, args: *const O, nargsf: usize, kw: O) -> O {
    // SAFETY: vectorcall arguments.
    unsafe {
        let g = &G_PARETO;
        if let Some([slf, a]) = bind(args, nargs(nargsf), kw, [std::ptr::null_mut(), ld(&V.alpha)])
            && !slf.is_null()
            && let Some(av) = operand(a)
            && av.is_finite()
            && av.abs() >= 0.0625
        {
            if !g.ready(func) {
                return call_original(func, args, nargsf, kw);
            }
            if native(slf) {
                let e = -1.0 / av;
                let r = with_cs(slf, || mt(slf).random());
                g.counters.hit_streak();
                return float(cv::pareto(r, e));
            }
        }
        g.counters.fallback_streak(Reason::Args);
        call_original(func, args, nargsf, kw)
    }
}

/// `Random.normalvariate(self, mu=0.0, sigma=1.0)`: `mu + z * sigma`.
unsafe extern "C" fn vc_normalvariate(func: O, args: *const O, nargsf: usize, kw: O) -> O {
    // SAFETY: vectorcall arguments.
    unsafe {
        let g = &G_NORMAL;
        let names = [std::ptr::null_mut(), ld(&V.mu), ld(&V.sigma)];
        if let Some([slf, mu, sigma]) = bind(args, nargs(nargsf), kw, names)
            && !slf.is_null()
            && let Some(m) = operand(or_default(func, mu, 0))
            && let Some(s) = operand(or_default(func, sigma, 1))
        {
            if !g.ready(func) {
                return call_original(func, args, nargsf, kw);
            }
            if native(slf) {
                let nv = f64::from_bits(NV_MAGICCONST.load(R));
                let z = normal_z(slf, nv);
                g.counters.hit_streak();
                return float(m + z * s);
            }
        }
        g.counters.fallback_streak(Reason::Args);
        call_original(func, args, nargsf, kw)
    }
}

/// Does `self.normalvariate` resolve to CPython's `Random.normalvariate`, still running
/// its validated code, with no tool listening to it? (Pure lookups.)
#[inline]
unsafe fn normal_is_ours(slf: O) -> bool {
    // SAFETY: attached thread; borrowed results compared only.
    unsafe {
        let f = ld(&NORMAL_FN);
        let name = ld(&V.normalvariate);
        !f.is_null()
            && _PyType_Lookup(ffi::Py_TYPE(slf), name) == f
            && (*f.cast::<ffi::PyFunctionObject>()).func_code == ld(&NORMAL_CODE)
            && accel::code_locally_quiet(ld(&NORMAL_CODE))
            && match INST.has_attr(slf, name) {
                Some(b) => !b,
                None => crate::targets::utils_kit::unshadowed(slf, name, f),
            }
    }
}

/// `Random.lognormvariate(self, mu, sigma)`: `_exp(self.normalvariate(mu, sigma))`.
unsafe extern "C" fn vc_lognormvariate(func: O, args: *const O, nargsf: usize, kw: O) -> O {
    // SAFETY: vectorcall arguments.
    unsafe {
        let g = &G_LOGNORM;
        let names = [std::ptr::null_mut(), ld(&V.mu), ld(&V.sigma)];
        if let Some([slf, mu, sigma]) = bind(args, nargs(nargsf), kw, names)
            && !slf.is_null()
            && let Some(m) = operand(mu)
            && let Some(s) = operand(sigma)
            // `exp` cannot overflow (an accepted |z| < 12.2; NaN/inf parameters fail this).
            && m + NORMAL_Z_BOUND * s.abs() <= 709.0
        {
            if !g.ready(func) {
                return call_original(func, args, nargsf, kw);
            }
            if native(slf) && normal_is_ours(slf) {
                let nv = f64::from_bits(NV_MAGICCONST.load(R));
                let z = normal_z(slf, nv);
                g.counters.hit_streak();
                return float(libm::exp(m + z * s));
            }
        }
        g.counters.fallback_streak(Reason::Args);
        call_original(func, args, nargsf, kw)
    }
}

/// Is the "inline values are valid" read calibrated? (Its own calibration, independent of
/// `utils_kit::INST`, which also needs `_PyDictKeys_StringLookup`: not exported on 3.14.)
static IV_OK: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// `Py_TPFLAGS_INLINE_VALUES` (3.13+).
const TPFLAGS_INLINE_VALUES: std::ffi::c_ulong = 1 << 2;

/// Calibrate [`inline_valid`] on `os.environ` (an `os._Environ`: a Python class whose
/// `__init__` sets five attributes; `os` is imported at startup), C calls only: its type
/// uses inline values (`struct _dictvalues { u8 capacity, size, embedded, valid; values[] }`
/// after `tp_basicsize`, CPython 3.13–3.15), the values are valid, `size` is 5 within
/// `capacity`, and the five attributes the generic getattr returns are the five slots.
fn calibrate_inline_valid(cx: &Prepare<'_>) -> Result<(), Refusal> {
    if IV_OK.load(R) {
        return Ok(());
    }
    let py = cx.py();
    let os = module_dict_s(cx, &key(py, "os"))?;
    let env = get_s(cx, &os, &key(py, "environ"))?;
    // SAFETY: layout reads of a live object after the flag check; the generic getattr of a
    // class without `__getattr__` runs no Python code.
    let ok = unsafe {
        let o = env.as_ptr();
        let tp = ffi::Py_TYPE(o);
        if ffi::PyType_GetFlags(tp) & TPFLAGS_INLINE_VALUES == 0 || !generic_getattro(tp) {
            false
        } else {
            let values = o.cast::<u8>().add((*tp).tp_basicsize as usize);
            let (cap, size, valid) = (*values, *values.add(1), *values.add(3));
            let slots = std::slice::from_raw_parts(values.add(8).cast::<O>(), usize::from(size.min(cap)));
            let mut found = 0;
            for n in ["_data", "encodekey", "decodekey", "encodevalue", "decodevalue"] {
                let v = ffi::PyObject_GenericGetAttr(o, key(py, n).as_ptr());
                if v.is_null() {
                    ffi::PyErr_Clear();
                    continue;
                }
                found += usize::from(slots.contains(&v));
                ffi::Py_DECREF(v);
            }
            valid == 1 && size == 5 && cap >= 5 && found == 5
        }
    };
    if ok {
        IV_OK.store(true, R);
    }
    Ok(())
}

/// Does `obj` keep its attributes in valid inline values? Then they are exactly the
/// non-NULL entries of its type's shared keys (all `str`), also when a `__dict__` was
/// materialized: a key outside the shared keys or a replaced `__dict__` marks the values
/// invalid (CPython 3.13–3.15, `Objects/dictobject.c`). A lookup of a `str` name then runs
/// no Python code (no `__eq__` of a foreign key).
#[inline(always)]
unsafe fn inline_valid(obj: O) -> bool {
    // SAFETY: calibrated layout, read only for inline-values types.
    unsafe {
        let tp = ffi::Py_TYPE(obj);
        IV_OK.load(R)
            && ffi::PyType_GetFlags(tp) & TPFLAGS_INLINE_VALUES != 0
            && std::ptr::read_volatile(obj.cast::<u8>().add((*tp).tp_basicsize as usize + 3)) == 1
    }
}

/// `self.gauss_next` can be read and written exactly like `LOAD_ATTR`/`STORE_ATTR` do,
/// without running Python code: the instance keeps its attributes in valid inline values,
/// the class has the generic `__setattr__` (the generic `__getattribute__` is checked by
/// `random_native`) and no `gauss_next` descriptor.
#[inline]
unsafe fn gauss_next_plain(slf: O) -> bool {
    // SAFETY: attached thread; borrowed lookups compared only.
    unsafe {
        let tp = ffi::Py_TYPE(slf);
        if (*tp).tp_setattro.map(|f| f as usize) != Some(ffi::PyObject_GenericSetAttr as *const () as usize)
            || !generic_getattro(tp)
            || !inline_valid(slf)
        {
            return false;
        }
        let c = _PyType_Lookup(tp, ld(&V.gauss_next));
        c.is_null() || {
            let ct = ffi::Py_TYPE(c);
            (*ct).tp_descr_get.is_none() && (*ct).tp_descr_set.is_none()
        }
    }
}

/// The body of `gauss` once the class and instance are checked: `Some(result)` (NULL with
/// an error CPython would raise at the same point), `None` to fall back (nothing
/// observable happened).
unsafe fn gauss(slf: O, m: f64, s: f64) -> Option<O> {
    // SAFETY: attached thread; generic attribute access on an inline-values instance.
    unsafe {
        let name = ld(&V.gauss_next);
        // z = self.gauss_next
        let z = ffi::PyObject_GenericGetAttr(slf, name);
        if z.is_null() {
            ffi::PyErr_Clear(); // AttributeError: CPython raises it (the original repeats)
            return None;
        }
        let zv = if z == ffi::Py_None() {
            None
        } else if is_type(z, &raw mut ffi::PyFloat_Type) {
            Some(ffi::PyFloat_AS_DOUBLE(z))
        } else {
            ffi::Py_DECREF(z);
            return None; // `mu + z * sigma` would run that object's operators
        };
        ffi::Py_DECREF(z);
        // self.gauss_next = None
        if ffi::PyObject_GenericSetAttr(slf, name, ffi::Py_None()) < 0 {
            return Some(std::ptr::null_mut());
        }
        let zf = match zv {
            Some(v) => v,
            None => {
                let twopi = f64::from_bits(TWOPI.load(R));
                let (a, b) = with_cs(slf, || mt(slf).random2());
                let (z, next) = cv::gauss_pair(a, b, twopi);
                let nf = float(next);
                if nf.is_null() {
                    return Some(nf);
                }
                let r = ffi::PyObject_GenericSetAttr(slf, name, nf);
                ffi::Py_DECREF(nf);
                if r < 0 {
                    return Some(std::ptr::null_mut());
                }
                z
            }
        };
        Some(float(m + zf * s))
    }
}

/// `Random.gauss(self, mu=0.0, sigma=1.0)`.
unsafe extern "C" fn vc_gauss(func: O, args: *const O, nargsf: usize, kw: O) -> O {
    // SAFETY: vectorcall arguments.
    unsafe {
        let g = &G_GAUSS;
        let names = [std::ptr::null_mut(), ld(&V.mu), ld(&V.sigma)];
        if let Some([slf, mu, sigma]) = bind(args, nargs(nargsf), kw, names)
            && !slf.is_null()
            && let Some(m) = operand(or_default(func, mu, 0))
            && let Some(s) = operand(or_default(func, sigma, 1))
        {
            if !g.ready(func) {
                return call_original(func, args, nargsf, kw);
            }
            if native(slf)
                && gauss_next_plain(slf)
                && let Some(r) = gauss(slf, m, s)
            {
                g.counters.hit_streak();
                return r;
            }
        }
        g.counters.fallback_streak(Reason::Args);
        call_original(func, args, nargsf, kw)
    }
}

// ---------------------------------------------------------------------------------------
// Preparation
// ---------------------------------------------------------------------------------------

/// Shared declarations of the five variate targets (`GroupSpec` "random.variates"): the
/// group's statics (idempotent), the module globals they read — `_log`, `_exp`, `_sqrt`,
/// `_cos`, `_sin` checked to be `math`'s functions, `TWOPI` to be `math.tau`,
/// `NV_MAGICCONST` an exact float — pinned into their own flag (so replacing one of them
/// suspends these five, not `randint`), and their values.
fn prepare_variates<'py>(py: Python<'py>, cx: &mut Prepare<'py>) -> Result<(), Refusal> {
    if super::MT_OK.load(R) == MT_BAD {
        return Err(Refusal::with(accel::Category::Selftest, super::SELFTEST_FAILED));
    }
    if LIBM_OK.load(R) == LIBM_BAD {
        return Err(Refusal::with(accel::Category::Selftest, LIBM_FAILED));
    }
    prepare_symbols(cx)?;
    let g = cx.module()?.dict();
    let cls = get_s(cx, &g, &key(py, "Random"))?.cast_into::<PyType>().map_err(PyErr::from)?;
    if cls.as_ptr() != ld(&RANDOM_CLS) {
        prepare_statics(py, cx, &cls)?;
    }
    calibrate_inline_valid(cx)?;
    let math = module_dict_s(cx, &key(py, "math"))?;
    let fns = [
        ("_log", "log", &LOG_FN),
        ("_exp", "exp", &EXP_FN),
        ("_sqrt", "sqrt", &SQRT_FN),
        ("_cos", "cos", &COS_FN),
        ("_sin", "sin", &SIN_FN),
        ("TWOPI", "tau", &TWOPI_OBJ),
    ];
    for (name, attr, slot) in fns {
        let k = key(py, name);
        let v = get_s(cx, &g, &k)?;
        if !v.is(&get_s(cx, &math, &key(py, attr))?) {
            return Err(Refusal::with(accel::Category::Deps, format!("random.{name} is not math.{attr}")));
        }
        cx.pin_s(&g, &k, &VARIATE_DEPS)?;
        keep(slot, &v);
    }
    let nv = get_s(cx, &g, &key(py, "NV_MAGICCONST"))?;
    let tau = get_s(cx, &math, &key(py, "tau"))?;
    // SAFETY: type checks and value reads of live objects.
    unsafe {
        if !is_type(nv.as_ptr(), &raw mut ffi::PyFloat_Type) || !is_type(tau.as_ptr(), &raw mut ffi::PyFloat_Type) {
            return Err(Refusal::new("random.NV_MAGICCONST / TWOPI are not floats"));
        }
        NV_MAGICCONST.store(ffi::PyFloat_AS_DOUBLE(nv.as_ptr()).to_bits(), R);
        TWOPI.store(ffi::PyFloat_AS_DOUBLE(tau.as_ptr()).to_bits(), R);
    }
    cx.pin_s(&g, &key(py, "NV_MAGICCONST"), &VARIATE_DEPS)?;
    for (slot, s) in [
        (&V.normalvariate, "normalvariate"),
        (&V.gauss_next, "gauss_next"),
        (&V.mu, "mu"),
        (&V.sigma, "sigma"),
        (&V.lambd, "lambd"),
        (&V.alpha, "alpha"),
    ] {
        if slot.load(R).is_null() {
            slot.store(interned(py, s), R);
        }
    }
    // `self.random` is resolved per call; `S.random` is the group's interned name.
    debug_assert!(!ld(&S.random).is_null());
    Ok(())
}

/// `math.tau` (kept alive with the others; `TWOPI` holds its value).
static TWOPI_OBJ: AtomicPtr<ffi::PyObject> = null!();

pub static VARIATES: accel::GroupSpec = accel::GroupSpec { name: "random.variates", prepare: prepare_variates };

const FP_NORMAL: accel::Fingerprints = &[(3, 14, "2e516c78feca5f98"), (3, 15, "afd4735486dc11b3")];

/// `lognormvariate` also stands in for `normalvariate`: its code is fingerprinted here and
/// its identity kept (the kernel requires `self.normalvariate` to be that function).
fn prep_lognorm<'py>(py: Python<'py>, cx: &mut Prepare<'py>) -> Result<(), Refusal> {
    // SAFETY: the class kept by the group's prepare (strong reference, attached thread).
    let cls = unsafe { Bound::from_borrowed_ptr(py, ld(&RANDOM_CLS)) }.cast_into::<PyType>().map_err(PyErr::from)?;
    let nf = cx
        .get_opt_s(&crate::accel::prepare::type_dict(&cls)?, &key(py, "normalvariate"))?
        .ok_or_else(|| Refusal::new("random.Random.normalvariate missing"))?;
    if nf.cast_exact::<pyo3::types::PyFunction>().is_err() {
        return Err(Refusal::new("random.Random.normalvariate is not a function"));
    }
    cx.fingerprint_code(&nf, FP_NORMAL)?; // the registry caches the hash per code object
    // SAFETY: an exact function object.
    let code = unsafe { (*nf.as_ptr().cast::<ffi::PyFunctionObject>()).func_code };
    keep(&NORMAL_FN, &nf);
    NORMAL_CODE.store(code, R);
    prepare_target(
        py,
        cx,
        &key(py, "lognormvariate"),
        &[(3, 14, "0c8006622d76a55f"), (3, 15, "a3cecf11d0f54952")],
        &G_LOGNORM,
        vc_lognormvariate,
    )
}

/// Net-benefit credits (contract §5.6) from the measured cells (docs/targets/random.md
/// §7.3): a fallback in mixed traffic costs the caller 330–440 Ir; a hit saves ~800 Ir
/// (`expovariate`, `paretovariate`: break-even at 31–35% hits, so a hit credits 2 and
/// the target stays installed while at least a third of the calls hit), ~1,300 Ir
/// (`gauss`: break-even 24%, credit 3) and ~3,100–3,400 Ir (`normalvariate`,
/// `lognormvariate`: break-even 10%, the default 4).
macro_rules! variate {
    ($spec:ident, $prep:ident, $name:literal, $guard:ident, $credit:expr) => {
        pub static $spec: TargetSpec = TargetSpec {
            name: $name,
            group: "random",
            module: Some("random"),
            versions: &[(3, 14), (3, 15)],
            free_threaded: true,
            counters: &$guard.counters,
            prepare: $prep,
            shared: Some(&VARIATES),
            hit_credit: $credit,
            ..TargetSpec::DEFAULT
        };
    };
    ($spec:ident, $prep:ident, $name:literal, $attr:literal, $kernel:ident, $guard:ident, $fp:expr, $credit:expr) => {
        fn $prep<'py>(py: Python<'py>, cx: &mut Prepare<'py>) -> Result<(), Refusal> {
            prepare_target(py, cx, &key(py, $attr), $fp, &$guard, $kernel)
        }
        variate!($spec, $prep, $name, $guard, $credit);
    };
}

variate!(
    NORMALVARIATE,
    prep_normal,
    "random.normalvariate",
    "normalvariate",
    vc_normalvariate,
    G_NORMAL,
    FP_NORMAL,
    crate::accel::stats::HIT_CREDIT
);
variate!(LOGNORMVARIATE, prep_lognorm, "random.lognormvariate", G_LOGNORM, crate::accel::stats::HIT_CREDIT);
variate!(
    EXPOVARIATE,
    prep_expo,
    "random.expovariate",
    "expovariate",
    vc_expovariate,
    G_EXPO,
    &[(3, 14, "94f27fe3a4af3bd2"), (3, 15, "8d8014d7d1775e3d")],
    2
);
variate!(
    PARETOVARIATE,
    prep_pareto,
    "random.paretovariate",
    "paretovariate",
    vc_paretovariate,
    G_PARETO,
    &[(3, 14, "1a130384d6ccb719"), (3, 15, "dce41532531c3cd2")],
    2
);
variate!(
    GAUSS,
    prep_gauss,
    "random.gauss",
    "gauss",
    vc_gauss,
    G_GAUSS,
    &[(3, 14, "faac491a958d3838"), (3, 15, "9bd699354dfa75aa")],
    3
);

/// The five variate targets.
pub static TARGETS: &[&TargetSpec] = &[&NORMALVARIATE, &LOGNORMVARIATE, &EXPOVARIATE, &PARETOVARIATE, &GAUSS];
