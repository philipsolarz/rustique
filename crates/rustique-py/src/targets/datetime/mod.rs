//! Group `datetime` (docs/targets/datetime.md): text parsing and formatting of dates, behind
//! the C entry points of `_datetime` and `time` (`ml_meth` sites: a fallback costs a few
//! instructions).
//!
//! | target | site | module (install trigger) |
//! |---|---|---|
//! | `datetime.datetime.strptime` / `datetime.date.strptime` / `datetime.time.strptime` | classmethods of `_datetime` | `_datetime` |
//! | `time.strptime` | `time.strptime` | `_datetime` (results need its types) |
//! | `datetime.date.strftime` (also `datetime.strftime`, `format()`, f-strings) | `date.strftime` | `_datetime` |
//! | `datetime.time.strftime` | `time.strftime` method | `_datetime` |
//! | `time.strftime` | `time.strftime` | `time` |
//!
//! The strptime kernels stand in for `_strptime`'s Python code: they fingerprint and watch
//! whichever `_strptime` module object `sys.modules` holds when they first see it (the first
//! call imports it), and fall back until it is in a validated steady state.

use std::sync::atomic::{AtomicPtr, AtomicU8, Ordering};

use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::PyType;

use crate::accel::{Prepare, Refusal, TargetSpec};

pub mod env;
mod fingerprints;
pub mod format;
pub mod parse;
mod ustr;
pub mod watch;

const VERSIONS: &[(u8, u8)] = &[(3, 14), (3, 15)];

static CAPI: AtomicPtr<ffi::PyDateTime_CAPI> = AtomicPtr::new(std::ptr::null_mut());
/// 0: not calibrated, 1: ok, 2: failed.
static TZ_CALIBRATED: AtomicU8 = AtomicU8::new(0);

/// The datetime C API (static types and constructors; process-global).
#[inline(always)]
pub fn capi() -> *mut ffi::PyDateTime_CAPI {
    CAPI.load(Ordering::Relaxed)
}

/// The C API from the already imported `_datetime` (never imports).
fn load_capi(cx: &Prepare<'_>) -> Result<(), Refusal> {
    if !capi().is_null() {
        return Ok(());
    }
    // The target's own module when `_datetime` triggered the install (it is not in
    // `sys.modules` while importlib re-inserts it), else the already imported one.
    let m = match cx.module() {
        Ok(m) if m.name().is_ok_and(|n| n == "_datetime") => m.clone(),
        _ => cx.loaded_module("_datetime")?,
    };
    let cap = cx.get(&m.dict(), "datetime_CAPI")?;
    // SAFETY: attached thread; the capsule name is checked by PyCapsule_GetPointer.
    let p = unsafe { ffi::PyCapsule_GetPointer(cap.as_ptr(), ffi::PyDateTime_CAPSULE_NAME.as_ptr()) };
    if p.is_null() {
        // SAFETY: attached thread.
        unsafe { ffi::PyErr_Clear() };
        return Err(Refusal::new("_datetime.datetime_CAPI is not the datetime C API capsule"));
    }
    let _ = cap.into_ptr(); // keep the capsule (and its static struct) referenced
    CAPI.store(p.cast(), Ordering::Relaxed);
    Ok(())
}

/// What every datetime target needs.
fn common_base(cx: &Prepare<'_>) -> Result<&'static env::Env, Refusal> {
    if !cfg!(all(target_os = "linux", target_env = "gnu")) {
        return Err(Refusal::new("glibc strftime/locale behaviour only validated on Linux/glibc"));
    }
    let e = env::capture(cx)?;
    ustr::calibrate(cx.py());
    Ok(e)
}

/// ... plus the datetime C API (every target but `time.strftime`).
fn common(cx: &Prepare<'_>) -> Result<&'static env::Env, Refusal> {
    let e = common_base(cx)?;
    load_capi(cx)?;
    if TZ_CALIBRATED.load(Ordering::Relaxed) == 0 {
        let ok = format::calibrate_timezone(cx.py())?;
        TZ_CALIBRATED.store(if ok { 1 } else { 2 }, Ordering::Relaxed);
    }
    Ok(e)
}

/// `cls` from the C API.
fn api_type<'py>(py: Python<'py>, t: *mut ffi::PyTypeObject) -> Bound<'py, PyType> {
    // SAFETY: a static type of the datetime C API.
    unsafe { Bound::from_borrowed_ptr(py, t.cast()).cast_into_unchecked::<PyType>() }
}

// ---------------------------------------------------------------------------------------
// strptime
// ---------------------------------------------------------------------------------------

fn prepare_classmethod_strptime(cx: &mut Prepare<'_>, kind: parse::Kind) -> Result<(), Refusal> {
    common(cx)?;
    parse::prepare_common(cx)?;
    let py = cx.py();
    let api = capi();
    // SAFETY: C API loaded by `common`.
    let (t, label) = unsafe {
        match kind {
            parse::Kind::DateTime => ((*api).DateTimeType, "datetime.datetime.strptime"),
            parse::Kind::Date => ((*api).DateType, "datetime.date.strptime"),
            _ => ((*api).TimeType, "datetime.time.strptime"),
        }
    };
    let desc = cx.type_attr(&api_type(py, t), "strptime")?;
    cx.fingerprint_c(&desc, fingerprints::c(label))?;
    let k = kind as usize;
    if cx.version().1 >= 15 {
        let kernel: parse::Fast = match kind {
            parse::Kind::DateTime => parse::datetime_fast,
            parse::Kind::Date => parse::date_fast,
            _ => parse::time_fast,
        };
        cx.ml_meth(&desc, kernel, &parse::ORIG_FAST[k])
    } else {
        let kernel: parse::VarArgs = match kind {
            parse::Kind::DateTime => parse::datetime_varargs,
            parse::Kind::Date => parse::date_varargs,
            _ => parse::time_varargs,
        };
        cx.ml_meth(&desc, kernel, &parse::ORIG_VAR[k])
    }
}

fn prepare_datetime_strptime(_py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    prepare_classmethod_strptime(cx, parse::Kind::DateTime)
}

fn prepare_date_strptime(_py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    prepare_classmethod_strptime(cx, parse::Kind::Date)
}

fn prepare_time_strptime(_py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    prepare_classmethod_strptime(cx, parse::Kind::Time)
}

fn prepare_time_fn_strptime(_py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    let e = common(cx)?;
    parse::prepare_common(cx)?;
    let py = cx.py();
    // SAFETY: captured module.
    let time = unsafe { env::obj(py, e.time_mod) }.cast_into::<pyo3::types::PyModule>().map_err(PyErr::from)?;
    let f = cx.get(&time.dict(), "strptime")?;
    cx.fingerprint_c(&f, fingerprints::c("time.strptime"))?;
    cx.ml_meth(&f, parse::time_fn as parse::VarArgs, &parse::ORIG_VAR[parse::Kind::TimeFn as usize])
}

// ---------------------------------------------------------------------------------------
// strftime
// ---------------------------------------------------------------------------------------

/// Kernel vs original on probe values (numeric directives: locale-independent).
fn strftime_self_test(cx: &mut Prepare<'_>, time_method: bool) -> Result<(), Refusal> {
    let what = if time_method { "time.strftime method" } else { "date.strftime" };
    cx.self_test(what, |py| {
        let api = capi();
        let mut ok = true;
        // SAFETY: C API loaded; kernels and originals take these arguments.
        unsafe {
            let objs: Vec<Bound<'_, PyAny>> = if time_method {
                vec![Bound::from_owned_ptr_or_err(
                    py,
                    ((*api).Time_FromTime)(7, 5, 9, 123, ffi::Py_None(), (*api).TimeType),
                )?]
            } else {
                vec![
                    Bound::from_owned_ptr_or_err(
                        py,
                        ((*api).DateTime_FromDateAndTime)(
                            2024,
                            2,
                            29,
                            23,
                            59,
                            58,
                            4321,
                            ffi::Py_None(),
                            (*api).DateTimeType,
                        ),
                    )?,
                    Bound::from_owned_ptr_or_err(py, ((*api).Date_FromDate)(987, 12, 31, (*api).DateType))?,
                ]
            };
            for o in &objs {
                for f in ["%Y-%m-%d %H:%M:%S.%f|%y %j %I %e %u %w %C %F %T %D %R %% %z%Z", ""] {
                    let fmt = pyo3::types::PyString::new(py, f);
                    let args = [fmt.as_ptr()];
                    let (k, orig) = if cx_version_15() {
                        let k = if time_method { format::time_fast } else { format::date_fast };
                        let og = if time_method { format::ORIG_TIME_FAST.get() } else { format::ORIG_DATE_FAST.get() };
                        (
                            k(o.as_ptr(), args.as_ptr(), 1, std::ptr::null_mut()),
                            og(o.as_ptr(), args.as_ptr(), 1, std::ptr::null_mut()),
                        )
                    } else {
                        let t = pyo3::types::PyTuple::new(py, [&fmt])?;
                        let k = if time_method { format::time_var } else { format::date_var };
                        let og = if time_method { format::ORIG_TIME_VAR.get() } else { format::ORIG_DATE_VAR.get() };
                        (
                            k(o.as_ptr(), t.as_ptr(), std::ptr::null_mut()),
                            og(o.as_ptr(), t.as_ptr(), std::ptr::null_mut()),
                        )
                    };
                    let k = Bound::from_owned_ptr_or_err(py, k)?;
                    let orig = Bound::from_owned_ptr_or_err(py, orig)?;
                    ok &= k.eq(&orig)?;
                }
            }
        }
        Ok(ok)
    })
}

fn cx_version_15() -> bool {
    crate::accel::version::running().1 >= 15
}

fn prepare_strftime_method(cx: &mut Prepare<'_>, time_method: bool) -> Result<(), Refusal> {
    common(cx)?;
    let py = cx.py();
    let api = capi();
    // SAFETY: C API loaded by `common`.
    let t = unsafe { if time_method { (*api).TimeType } else { (*api).DateType } };
    let label = if time_method { "datetime.time.strftime" } else { "datetime.date.strftime" };
    let desc = cx.type_attr(&api_type(py, t), "strftime")?;
    cx.fingerprint_c(&desc, fingerprints::c(label))?;
    if cx.version().1 >= 15 {
        if time_method {
            cx.ml_meth(&desc, format::time_fast as format::FastKw, &format::ORIG_TIME_FAST)?;
        } else {
            cx.ml_meth(&desc, format::date_fast as format::FastKw, &format::ORIG_DATE_FAST)?;
        }
    } else if time_method {
        cx.ml_meth(&desc, format::time_var as format::VarKw, &format::ORIG_TIME_VAR)?;
    } else {
        cx.ml_meth(&desc, format::date_var as format::VarKw, &format::ORIG_DATE_VAR)?;
    }
    strftime_self_test(cx, time_method)
}

fn prepare_date_strftime(_py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    prepare_strftime_method(cx, false)
}

fn prepare_time_strftime(_py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    prepare_strftime_method(cx, true)
}

fn prepare_time_fn_strftime(_py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    let e = common_base(cx)?;
    let py = cx.py();
    // SAFETY: captured module.
    let time = unsafe { env::obj(py, e.time_mod) }.cast_into::<pyo3::types::PyModule>().map_err(PyErr::from)?;
    if !cx.module()?.is(&time) {
        return Err(Refusal::new("time module differs from the one captured"));
    }
    let f = cx.get(&time.dict(), "strftime")?;
    cx.fingerprint_c(&f, fingerprints::c("time.strftime"))?;
    cx.ml_meth(&f, format::time_fn as format::VarArgs, &format::ORIG_FN)?;
    cx.self_test("time.strftime", |py| {
        let fmt = "%Y-%m-%d %H:%M:%S|%y %j %I %e %u %w %C %F %T %D %R %%";
        let args = pyo3::types::PyTuple::new(
            py,
            [fmt.into_pyobject(py)?.into_any(), (2024, 2, 29, 23, 59, 58, 3, 60, -1).into_pyobject(py)?.into_any()],
        )?;
        // SAFETY: METH_VARARGS calls with a valid tuple.
        unsafe {
            let k = Bound::from_owned_ptr_or_err(py, format::time_fn(time.as_ptr(), args.as_ptr()))?;
            let o = Bound::from_owned_ptr_or_err(py, format::ORIG_FN.get()(time.as_ptr(), args.as_ptr()))?;
            k.eq(&o)
        }
    })
}

// ---------------------------------------------------------------------------------------
// Specs
// ---------------------------------------------------------------------------------------

pub static DATETIME_STRPTIME: TargetSpec = TargetSpec {
    name: "datetime.datetime.strptime",
    group: "datetime",
    module: Some("_datetime"),
    versions: VERSIONS,
    free_threaded: true,
    checkable: true,
    counters: &parse::COUNTERS[0],
    prepare: prepare_datetime_strptime,
    ..TargetSpec::DEFAULT
};

pub static DATE_STRPTIME: TargetSpec = TargetSpec {
    name: "datetime.date.strptime",
    group: "datetime",
    module: Some("_datetime"),
    versions: VERSIONS,
    free_threaded: true,
    checkable: true,
    counters: &parse::COUNTERS[1],
    prepare: prepare_date_strptime,
    ..TargetSpec::DEFAULT
};

pub static TIME_STRPTIME: TargetSpec = TargetSpec {
    name: "datetime.time.strptime",
    group: "datetime",
    module: Some("_datetime"),
    versions: VERSIONS,
    free_threaded: true,
    checkable: true,
    counters: &parse::COUNTERS[2],
    prepare: prepare_time_strptime,
    ..TargetSpec::DEFAULT
};

pub static TIME_FN_STRPTIME: TargetSpec = TargetSpec {
    name: "time.strptime",
    group: "datetime",
    module: Some("_datetime"),
    versions: VERSIONS,
    free_threaded: true,
    checkable: true,
    counters: &parse::COUNTERS[3],
    prepare: prepare_time_fn_strptime,
    ..TargetSpec::DEFAULT
};

pub static DATE_STRFTIME: TargetSpec = TargetSpec {
    name: "datetime.date.strftime",
    group: "datetime",
    module: Some("_datetime"),
    versions: VERSIONS,
    free_threaded: true,
    checkable: true,
    counters: &format::C_DATE,
    prepare: prepare_date_strftime,
    ..TargetSpec::DEFAULT
};

pub static TIME_STRFTIME: TargetSpec = TargetSpec {
    name: "datetime.time.strftime",
    group: "datetime",
    module: Some("_datetime"),
    versions: VERSIONS,
    free_threaded: true,
    checkable: true,
    counters: &format::C_TIME,
    prepare: prepare_time_strftime,
    ..TargetSpec::DEFAULT
};

pub static TIME_FN_STRFTIME: TargetSpec = TargetSpec {
    name: "time.strftime",
    group: "datetime",
    module: Some("time"),
    versions: VERSIONS,
    free_threaded: true,
    checkable: true,
    counters: &format::C_FN,
    prepare: prepare_time_fn_strftime,
    ..TargetSpec::DEFAULT
};

/// The spec of a strptime kernel.
pub(crate) fn spec(kind: &parse::Kind) -> &'static TargetSpec {
    match kind {
        parse::Kind::DateTime => &DATETIME_STRPTIME,
        parse::Kind::Date => &DATE_STRPTIME,
        parse::Kind::Time => &TIME_STRPTIME,
        parse::Kind::TimeFn => &TIME_FN_STRPTIME,
    }
}

pub static TARGETS: &[&TargetSpec] = &[
    &DATETIME_STRPTIME,
    &DATE_STRPTIME,
    &TIME_STRPTIME,
    &TIME_FN_STRPTIME,
    &DATE_STRFTIME,
    &TIME_STRFTIME,
    &TIME_FN_STRFTIME,
];

/// `_accel._datetime_debug()`: the group's validation state (tests, diagnostics).
#[pyfunction]
fn _datetime_debug(py: Python<'_>) -> PyResult<Bound<'_, pyo3::types::PyDict>> {
    let d = pyo3::types::PyDict::new(py);
    d.set_item("generation", watch::current())?;
    let (sp_ok, sp_bad, last) = parse::validation_state();
    d.set_item("strptime_ok_gen", sp_ok)?;
    d.set_item("strptime_bad_gen", sp_bad)?;
    d.set_item("strptime_last_failure", last)?;
    let (ft_ok, ft_bad) = format::validation_state();
    d.set_item("strftime_ok_gen", ft_ok)?;
    d.set_item("strftime_bad_gen", ft_bad)?;
    d.set_item("lc_time_is_c", env::lc_time_is_c())?;
    d.set_item("thread_locale_is_global", env::thread_locale_is_global())?;
    Ok(d)
}

pub fn register(accel: &Bound<'_, pyo3::types::PyModule>) -> PyResult<()> {
    accel.add_function(wrap_pyfunction!(_datetime_debug, accel)?)
}
