//! `datetime.strptime`, `date.strptime`, `time.strptime` (classmethods of `_datetime`) and
//! the function `time.strptime`: C entry points whose originals import `_strptime` and run
//! its Python code. The kernels parse in Rust (`rustique_core::datetime::strptime`) when the
//! call is in the exact subset and `_strptime` is in a steady state in which its original
//! would neither rebuild its caches nor do anything else observable; otherwise they call
//! the original with the unchanged arguments.

use std::cell::RefCell;
use std::ffi::c_int;
use std::rc::Rc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList, PyString, PyTuple};

use rustique_core::datetime::strptime::{self, A_MONTH, A_WEEKDAY, AM_PM, F_MONTH, F_WEEKDAY, Parsed, Program, Version};

use super::env::{self, Env, item, obj};
use super::{fingerprints, ustr, watch};
use crate::accel::{Counters, Original, Prepare, Reason, Refusal, Tools, code_locally_quiet};

/// Which entry point.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum Kind {
    DateTime = 0,
    Date = 1,
    Time = 2,
    TimeFn = 3,
}

pub static COUNTERS: [Counters; 4] = [const { Counters::new() }; 4];
/// Original `ml_meth` per [`Kind`]: METH_FASTCALL classmethods (3.15) ...
pub static ORIG_FAST: [Original<Fast>; 3] = [const { Original::new() }; 3];
/// ... METH_VARARGS classmethods (3.14) and the function `time.strptime` (index 3).
pub static ORIG_VAR: [Original<VarArgs>; 4] = [const { Original::new() }; 4];
/// The static types `cls` must be (DateTime, Date, Time), from the datetime C API.
pub static TYPES: [AtomicUsize; 3] = [const { AtomicUsize::new(0) }; 3];
static VERSION: AtomicUsize = AtomicUsize::new(0); // 14 / 15

static TOOLS: Tools = Tools::new();

// ---------------------------------------------------------------------------------------
// `_strptime` dependencies (captured when `_strptime` is first seen fully imported)
// ---------------------------------------------------------------------------------------

/// Functions the original runs (per call, their code objects must be unchanged and free
/// of local monitoring events). Indices: wrappers per [`Kind`], then the shared ones.
const FUNCS: [&str; 7] = [
    "_strptime_datetime_datetime",
    "_strptime_datetime_date",
    "_strptime_datetime_time",
    "_strptime_time",
    "_strptime",
    "_getlang",
    "_parse_tz",
];
const LOCALE_FUNCS: [&str; 3] = ["getlocale", "_parse_localename", "normalize"];
/// Also fingerprinted (they build `_TimeRE_cache`).
const CLASS_METHODS: [(&str, &str); 9] = [
    ("TimeRE", "__init__"),
    ("TimeRE", "_TimeRE__seqToRE"),
    ("TimeRE", "pattern"),
    ("TimeRE", "compile"),
    ("LocaleTime", "__init__"),
    ("LocaleTime", "_LocaleTime__calc_weekday"),
    ("LocaleTime", "_LocaleTime__calc_month"),
    ("LocaleTime", "_LocaleTime__calc_am_pm"),
    ("LocaleTime", "_LocaleTime__calc_alt_digits"),
];

#[derive(Debug)]
pub struct SpDeps {
    module: usize,
    dict: usize,
    /// (function, code) for FUNCS then LOCALE_FUNCS.
    codes: [(usize, usize); 10],
    time_defaults: usize,
    locale_mod: usize,
    locale_dict: usize,
    locale_alias: usize,
    setlocale: usize,
    time_re_cls: usize,
    locale_time_cls: usize,
    cache_lock_type: usize,
}

static SP: OnceLock<SpDeps> = OnceLock::new();
static SP_OK: AtomicU64 = AtomicU64::new(0);
static SP_BAD: AtomicU64 = AtomicU64::new(0);

fn keep(o: &Bound<'_, PyAny>) -> usize {
    o.clone().into_ptr() as usize
}

/// Capture and fingerprint `_strptime` (the target's module) and `locale`, once. Runs in
/// `prepare` (maybe inside the `sys.modules` watcher): dict reads and C getters only.
fn capture_sp(cx: &mut Prepare<'_>) -> Result<&'static SpDeps, Refusal> {
    let m = cx.module()?.clone();
    if let Some(sp) = SP.get() {
        if sp.module != m.as_ptr() as usize {
            return Err(Refusal::new("_strptime was re-imported (a different module object)"));
        }
        return Ok(sp);
    }
    let py = cx.py();
    let locale = cx.loaded_module("locale")?;
    let clocale = cx.loaded_module("_locale")?;
    let d = m.dict();
    let ld = locale.dict();
    let mut codes = [(0usize, 0usize); 10];
    for (i, name) in FUNCS.iter().chain(LOCALE_FUNCS.iter()).enumerate() {
        let f = if i < FUNCS.len() { cx.get(&d, name)? } else { cx.get(&ld, name)? };
        if f.cast_exact::<pyo3::types::PyFunction>().is_err() {
            return Err(Refusal::from(format!("{name} is not a Python function")));
        }
        cx.fingerprint_code(&f, fingerprints::code(name))?;
        let code = f.getattr("__code__")?;
        if !crate::accel::version::code_layout_ok_for(&code)? {
            return Err(Refusal::new("PyCodeObject layout differs"));
        }
        codes[i] = (keep(&f), keep(&code));
    }
    cx.fingerprint_code(&cx.get(&d, "_fixmonths")?, fingerprints::code("_fixmonths"))?;
    for (cls, meth) in CLASS_METHODS {
        let ty = cx.get(&d, cls)?.cast_into::<pyo3::types::PyType>().map_err(PyErr::from)?;
        let f = cx.type_attr(&ty, meth)?;
        cx.fingerprint_code(&f, fingerprints::code(&format!("{cls}.{meth}")))?;
    }
    let setlocale = cx.get(&ld, "_setlocale")?;
    if !setlocale.is(&cx.get(&clocale.dict(), "setlocale")?) {
        return Err(Refusal::new("locale._setlocale is not _locale.setlocale"));
    }
    let time_strptime = cx.get(&d, "_strptime_time")?;
    let defaults = time_strptime.getattr("__defaults__")?;
    if !defaults.eq(("%a %b %d %H:%M:%S %Y",))? {
        return Err(Refusal::new("_strptime_time default format differs"));
    }
    let sp = SpDeps {
        module: keep(m.as_any()),
        dict: keep(d.as_any()),
        codes,
        time_defaults: keep(&defaults),
        locale_mod: keep(locale.as_any()),
        locale_dict: keep(ld.as_any()),
        locale_alias: keep(&cx.get(&ld, "locale_alias")?),
        setlocale: keep(&setlocale),
        time_re_cls: keep(&cx.get(&d, "TimeRE")?),
        locale_time_cls: keep(&cx.get(&d, "LocaleTime")?),
        cache_lock_type: keep(cx.get(&d, "_cache_lock")?.get_type().as_any()),
    };
    // SAFETY: just captured.
    for dict in [sp.dict, sp.locale_dict, sp.locale_alias] {
        let dict = unsafe { obj(py, dict) }.cast_into::<PyDict>().map_err(PyErr::from)?;
        watch::watch(&dict, None)?;
    }
    Ok(SP.get_or_init(|| sp))
}

const SHADOWS: [&str; 10] = ["isinstance", "enumerate", "str", "len", "int", "tuple", "ValueError", "TypeError", "KeyError", "range"];

fn list_is(o: &Bound<'_, PyAny>, expected: &[&str], leading_empty: bool) -> bool {
    let Ok(l) = o.cast::<PyList>() else { return false };
    if !l.is_exact_instance_of::<PyList>() || l.len() != expected.len() + usize::from(leading_empty) {
        return false;
    }
    let want = leading_empty.then_some("").into_iter().chain(expected.iter().copied());
    l.iter().zip(want).all(|(x, w)| x.is_exact_instance_of::<PyString>() && x.eq(w).unwrap_or(false))
}

/// Everything the original reads in its steady state is what the kernel assumes.
fn validate(py: Python<'_>, e: &Env) -> PyResult<bool> {
    if !env::common_ok(py, e) {
        return Ok(false);
    }
    let Some(sp) = SP.get() else { return Ok(false) };
    // SAFETY: captured objects.
    let (modules, d, tdict, ldict) = unsafe {
        (
            obj(py, e.sys_modules).cast_into::<PyDict>()?,
            obj(py, sp.dict).cast_into::<PyDict>()?,
            obj(py, e.time_dict).cast_into::<PyDict>()?,
            obj(py, sp.locale_dict).cast_into::<PyDict>()?,
        )
    };
    let is = |o: Option<Bound<'_, PyAny>>, addr: usize| o.is_some_and(|o| o.as_ptr() as usize == addr);
    // sys.modules['_strptime'] and the module's globals.
    // SAFETY: captured module.
    if !is(item(&modules, "_strptime"), sp.module) || unsafe { ffi::Py_TYPE(sp.module as *mut _) } as usize != e.module_type {
        return Ok(false);
    }
    for (i, name) in FUNCS.iter().enumerate() {
        if !is(item(&d, name), sp.codes[i].0) {
            return Ok(false);
        }
    }
    for (i, name) in LOCALE_FUNCS.iter().enumerate() {
        if !is(item(&ldict, name), sp.codes[FUNCS.len() + i].0) {
            return Ok(false);
        }
    }
    let api = super::capi();
    // SAFETY: datetime C API, filled at install.
    let (date_t, delta_t, tz_t) = unsafe {
        ((*api).DateType as usize, (*api).DeltaType as usize, ffi::Py_TYPE((*api).TimeZone_UTC) as usize)
    };
    if !(is(item(&d, "time"), e.time_mod)
        && is(item(&d, "locale"), sp.locale_mod)
        && is(item(&d, "datetime_date"), date_t)
        && is(item(&d, "datetime_timedelta"), delta_t)
        && is(item(&d, "datetime_timezone"), tz_t)
        && is(item(&d, "TimeRE"), sp.time_re_cls)
        && is(item(&d, "LocaleTime"), sp.locale_time_cls)
        && item(&d, "_cache_lock").is_some_and(|l| l.get_type().as_ptr() as usize == sp.cache_lock_type)
        && is(item(&ldict, "_setlocale"), sp.setlocale)
        && is(item(&ldict, "locale_alias"), sp.locale_alias)
        && item(&ldict, "LC_TIME").is_some_and(|v| v.is_exact_instance_of::<pyo3::types::PyInt>() && v.eq(2).unwrap_or(false))
        && item(&ldict, "LC_ALL").is_some_and(|v| v.is_exact_instance_of::<pyo3::types::PyInt>() && v.eq(6).unwrap_or(false)))
    {
        return Ok(false);
    }
    // SAFETY: captured module.
    if unsafe { ffi::Py_TYPE(sp.locale_mod as *mut _) } as usize != e.module_type {
        return Ok(false);
    }
    if SHADOWS.iter().any(|n| item(&d, n).is_some() || item(&ldict, n).is_some()) {
        return Ok(false);
    }
    // SAFETY: captured dict.
    let alias = unsafe { obj(py, sp.locale_alias) }.cast_into::<PyDict>()?;
    if !item(&alias, "c").is_some_and(|v| v.is_exact_instance_of::<PyString>() && v.eq("C").unwrap_or(false)) {
        return Ok(false);
    }
    // time module values `_strptime` reads.
    if !item(&tdict, "_STRUCT_TM_ITEMS").is_some_and(|v| v.is_exact_instance_of::<pyo3::types::PyInt>() && v.eq(11).unwrap_or(false)) {
        return Ok(false);
    }
    // The cached LocaleTime: C locale content, and the original's recreation test
    // (`_getlang() != lang or time.tzname != tzname or time.daylight != daylight`) is false
    // while LC_TIME is "C" (checked per call).
    let Some(cache) = item(&d, "_TimeRE_cache") else { return Ok(false) };
    if cache.get_type().as_ptr() as usize != sp.time_re_cls {
        return Ok(false);
    }
    let lt = cache.getattr("locale_time")?;
    if lt.get_type().as_ptr() as usize != sp.locale_time_cls {
        return Ok(false);
    }
    let lang = lt.getattr("lang")?;
    if !(lang.is_exact_instance_of::<PyTuple>() && lang.eq((py.None(), py.None()))?) {
        return Ok(false);
    }
    let (Some(tzname), Some(daylight)) = (item(&tdict, "tzname"), item(&tdict, "daylight")) else { return Ok(false) };
    let lt_tz = lt.getattr("tzname")?;
    let lt_dl = lt.getattr("daylight")?;
    if !(env::is_str_tuple(&tzname) && env::is_str_tuple(&lt_tz) && tzname.eq(&lt_tz)?) {
        return Ok(false);
    }
    let int_t = py.get_type::<pyo3::types::PyInt>();
    if !(daylight.is_exact_instance(&int_t) && lt_dl.is_exact_instance(&int_t) && daylight.eq(&lt_dl)?) {
        return Ok(false);
    }
    let alt = lt.getattr("LC_alt_digits")?;
    Ok(alt.is_exact_instance_of::<PyTuple>()
        && alt.len()? == 0
        && list_is(&lt.getattr("a_weekday")?, &A_WEEKDAY, false)
        && list_is(&lt.getattr("f_weekday")?, &F_WEEKDAY, false)
        && list_is(&lt.getattr("a_month")?, &A_MONTH, true)
        && list_is(&lt.getattr("f_month")?, &F_MONTH, true)
        && list_is(&lt.getattr("am_pm")?, &AM_PM, false))
}

/// Is `_strptime` in the validated steady state? One relaxed load in the common case.
#[inline(always)]
fn ready(py: Python<'_>, e: &Env) -> bool {
    let g = watch::current();
    if g == SP_OK.load(Ordering::Relaxed) {
        return true;
    }
    if g == SP_BAD.load(Ordering::Relaxed) {
        return false;
    }
    revalidate(py, e, g)
}

#[cold]
#[inline(never)]
fn revalidate(py: Python<'_>, e: &Env, g: u64) -> bool {
    let ok = validate(py, e).unwrap_or(false);
    if ok {
        SP_OK.store(g, Ordering::Relaxed);
    } else {
        SP_BAD.store(g, Ordering::Relaxed);
    }
    ok
}

/// The original's Python functions for this call are unchanged and nobody listens locally.
///
/// # Safety
/// Attached thread; `sp` captured.
#[inline(always)]
unsafe fn codes_ok(sp: &SpDeps, kind: Kind, with_tz: bool) -> bool {
    let check = |i: usize| {
        let (f, c) = sp.codes[i];
        // SAFETY: captured live function; `func_code` read (atomic pointer-sized field).
        unsafe {
            let code = (*(f as *mut ffi::PyFunctionObject)).func_code;
            code as usize == c && code_locally_quiet(code)
        }
    };
    check(kind as usize) && check(4) && check(5) && check(7) && check(8) && check(9) && (!with_tz || check(6))
}

// ---------------------------------------------------------------------------------------
// Compiled-format cache (per thread: no locks)
// ---------------------------------------------------------------------------------------

thread_local! {
    static PROGS: RefCell<Vec<(Box<[u8]>, Option<Rc<Program>>)>> = const { RefCell::new(Vec::new()) };
}

fn program(fmt: &[u8]) -> Option<Rc<Program>> {
    PROGS.with(|c| {
        let mut c = c.borrow_mut();
        if let Some((_, p)) = c.iter().find(|(f, _)| &**f == fmt) {
            return p.clone();
        }
        let v = if VERSION.load(Ordering::Relaxed) == 14 { Version::V3_14 } else { Version::V3_15 };
        let p = strptime::compile(fmt, v).map(Rc::new);
        if c.len() >= 8 {
            c.clear();
        }
        c.push((fmt.into(), p.clone()));
        p
    })
}

// ---------------------------------------------------------------------------------------
// Kernels
// ---------------------------------------------------------------------------------------

pub type Fast = unsafe extern "C" fn(*mut ffi::PyObject, *const *mut ffi::PyObject, ffi::Py_ssize_t) -> *mut ffi::PyObject;
pub type VarArgs = unsafe extern "C" fn(*mut ffi::PyObject, *mut ffi::PyObject) -> *mut ffi::PyObject;

const MAX_MICROS: i64 = 86_400_000_000;

/// The whole decision and the result; `None` = call the original (nothing observable done).
///
/// # Safety
/// Attached thread; `s`/`fmt` live objects.
#[inline(always)]
unsafe fn run(kind: Kind, cls: *mut ffi::PyObject, s: *mut ffi::PyObject, fmt: *mut ffi::PyObject) -> Option<*mut ffi::PyObject> {
    let c = &COUNTERS[kind as usize];
    // SAFETY: attached thread throughout; objects are live.
    unsafe {
        if kind != Kind::TimeFn && cls as usize != TYPES[kind as usize].load(Ordering::Relaxed) {
            c.fallback(Reason::Args);
            return None;
        }
        if ffi::PyUnicode_CheckExact(s) == 0 || ffi::PyUnicode_CheckExact(fmt) == 0 {
            c.fallback(Reason::Args);
            return None;
        }
        let Some(e) = env::env() else { return None };
        if !env::frame_ok(e) {
            c.fallback(Reason::Interp);
            return None;
        }
        let py = Python::assume_attached();
        if !ready(py, e) || !env::lc_time_is_c() {
            c.fallback(Reason::Deps);
            return None;
        }
        let sp = SP.get()?;
        if !TOOLS.quiet() || !codes_ok(sp, kind, false) {
            c.fallback(Reason::Tools);
            return None;
        }
        let (Some(fb), Some(sb)) = (ustr::ascii(fmt), ustr::ascii(s)) else {
            c.fallback(Reason::Args);
            return None;
        };
        let Some(p) = program(fb).and_then(|prog| prog.parse(sb)) else {
            c.fallback(Reason::Args);
            return None;
        };
        let r = build(kind, &p, sp);
        match r {
            Some(_) => c.hit(),
            None => c.fallback(Reason::Args),
        }
        r
    }
}

/// `timezone(timedelta(seconds=gmtoff, microseconds=fraction))`, or `None` if CPython raises.
unsafe fn tz(p: &Parsed) -> Option<Option<*mut ffi::PyObject>> {
    let Some(off) = p.gmtoff else { return Some(None) };
    let total = off * 1_000_000 + p.gmtoff_fraction;
    if total.abs() >= MAX_MICROS {
        return None; // timezone() raises ValueError
    }
    // SAFETY: datetime C API is filled; attached thread.
    unsafe {
        let api = super::capi();
        let delta = ((*api).Delta_FromDelta)(0, off as c_int, p.gmtoff_fraction as c_int, 1, (*api).DeltaType);
        if delta.is_null() {
            return Some(Some(std::ptr::null_mut()));
        }
        let t = ((*api).TimeZone_FromTimeZone)(delta, std::ptr::null_mut());
        ffi::Py_DECREF(delta);
        Some(Some(t))
    }
}

/// Build what the original returns.
unsafe fn build(kind: Kind, p: &Parsed, sp: &SpDeps) -> Option<*mut ffi::PyObject> {
    // SAFETY: attached thread; datetime C API filled.
    unsafe {
        let api = super::capi();
        match kind {
            Kind::Date => {
                if !rustique_core::datetime::cal::valid_date(p.year, p.month, p.day) {
                    return None;
                }
                Some(((*api).Date_FromDate)(p.year as c_int, p.month as c_int, p.day as c_int, (*api).DateType))
            }
            Kind::DateTime | Kind::Time => {
                if p.second > 59 || (kind == Kind::DateTime && !rustique_core::datetime::cal::valid_date(p.year, p.month, p.day)) {
                    return None;
                }
                if p.gmtoff.is_some() && !codes_ok(sp, kind, true) {
                    return None;
                }
                let tzinfo = tz(p)?;
                let tzo = match tzinfo {
                    Some(t) if t.is_null() => return Some(t),
                    Some(t) => t,
                    None => ffi::Py_None(),
                };
                let r = if kind == Kind::DateTime {
                    ((*api).DateTime_FromDateAndTime)(
                        p.year as c_int,
                        p.month as c_int,
                        p.day as c_int,
                        p.hour as c_int,
                        p.minute as c_int,
                        p.second as c_int,
                        p.fraction as c_int,
                        tzo,
                        (*api).DateTimeType,
                    )
                } else {
                    ((*api).Time_FromTime)(
                        p.hour as c_int,
                        p.minute as c_int,
                        p.second as c_int,
                        p.fraction as c_int,
                        tzo,
                        (*api).TimeType,
                    )
                };
                if tzinfo.is_some() {
                    ffi::Py_DECREF(tzo);
                }
                Some(r)
            }
            Kind::TimeFn => {
                let e = env::env()?;
                let st = ffi::PyStructSequence_New(e.struct_time as *mut ffi::PyTypeObject);
                if st.is_null() {
                    return Some(st);
                }
                let vals: [i64; 8] = [
                    p.year,
                    i64::from(p.month),
                    i64::from(p.day),
                    i64::from(p.hour),
                    i64::from(p.minute),
                    i64::from(p.second),
                    i64::from(p.weekday),
                    p.julian,
                ];
                for (i, v) in vals.iter().enumerate() {
                    let o = ffi::PyLong_FromLongLong(*v);
                    if o.is_null() {
                        ffi::Py_DECREF(st);
                        return Some(std::ptr::null_mut());
                    }
                    ffi::PyStructSequence_SetItem(st, i as ffi::Py_ssize_t, o);
                }
                let minus1 = ffi::PyLong_FromLong(-1);
                ffi::PyStructSequence_SetItem(st, 8, minus1);
                ffi::PyStructSequence_SetItem(st, 9, ffi::Py_NewRef(ffi::Py_None()));
                let off = match p.gmtoff {
                    Some(g) => ffi::PyLong_FromLongLong(g),
                    None => ffi::Py_NewRef(ffi::Py_None()),
                };
                if minus1.is_null() || off.is_null() {
                    if !off.is_null() {
                        ffi::Py_DECREF(off);
                    }
                    ffi::Py_DECREF(st);
                    return Some(std::ptr::null_mut());
                }
                ffi::PyStructSequence_SetItem(st, 10, off);
                Some(st)
            }
        }
    }
}

macro_rules! classmethod_kernels {
    ($fast:ident, $var:ident, $kind:expr) => {
        /// METH_FASTCALL | METH_CLASS (3.15).
        pub unsafe extern "C" fn $fast(
            cls: *mut ffi::PyObject,
            args: *const *mut ffi::PyObject,
            nargs: ffi::Py_ssize_t,
        ) -> *mut ffi::PyObject {
            // SAFETY: CPython's calling convention; fallback with unchanged arguments.
            unsafe {
                if nargs == 2 {
                    if let Some(r) = run($kind, cls, *args, *args.add(1)) {
                        return r;
                    }
                } else {
                    COUNTERS[$kind as usize].fallback(Reason::Args);
                }
                ORIG_FAST[$kind as usize].get()(cls, args, nargs)
            }
        }

        /// METH_VARARGS | METH_CLASS (3.14).
        pub unsafe extern "C" fn $var(cls: *mut ffi::PyObject, args: *mut ffi::PyObject) -> *mut ffi::PyObject {
            // SAFETY: CPython's calling convention; fallback with unchanged arguments.
            unsafe {
                if ffi::PyTuple_GET_SIZE(args) == 2 {
                    if let Some(r) = run($kind, cls, ffi::PyTuple_GET_ITEM(args, 0), ffi::PyTuple_GET_ITEM(args, 1)) {
                        return r;
                    }
                } else {
                    COUNTERS[$kind as usize].fallback(Reason::Args);
                }
                ORIG_VAR[$kind as usize].get()(cls, args)
            }
        }
    };
}

classmethod_kernels!(datetime_fast, datetime_varargs, Kind::DateTime);
classmethod_kernels!(date_fast, date_varargs, Kind::Date);
classmethod_kernels!(time_fast, time_varargs, Kind::Time);

/// `time.strptime(string[, format])` (METH_VARARGS on 3.14 and 3.15).
pub unsafe extern "C" fn time_fn(module: *mut ffi::PyObject, args: *mut ffi::PyObject) -> *mut ffi::PyObject {
    // SAFETY: CPython's calling convention; fallback with unchanged arguments.
    unsafe {
        let n = ffi::PyTuple_GET_SIZE(args);
        let e = env::env();
        if e.is_some_and(|e| e.time_mod == module as usize) && (n == 2 || n == 1) {
            let s = ffi::PyTuple_GET_ITEM(args, 0);
            let fmt = if n == 2 {
                Some(ffi::PyTuple_GET_ITEM(args, 1))
            } else {
                // `_strptime_time`'s default, read from its (unchanged) __defaults__.
                SP.get().and_then(|sp| {
                    let f = sp.codes[Kind::TimeFn as usize].0 as *mut ffi::PyFunctionObject;
                    let d = (*f).func_defaults;
                    (d as usize == sp.time_defaults && ffi::PyTuple_GET_SIZE(d) == 1).then(|| ffi::PyTuple_GET_ITEM(d, 0))
                })
            };
            if let Some(fmt) = fmt
                && let Some(r) = run(Kind::TimeFn, module, s, fmt)
            {
                return r;
            }
        } else {
            COUNTERS[Kind::TimeFn as usize].fallback(Reason::Args);
        }
        ORIG_VAR[Kind::TimeFn as usize].get()(module, args)
    }
}

/// Setup shared by the strptime targets (their module is `_strptime`).
pub fn prepare_common(cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    let (_, minor) = cx.version();
    VERSION.store(usize::from(minor), Ordering::Relaxed);
    let api = super::capi();
    // SAFETY: datetime C API filled by the caller.
    unsafe {
        TYPES[0].store((*api).DateTimeType as usize, Ordering::Relaxed);
        TYPES[1].store((*api).DateType as usize, Ordering::Relaxed);
        TYPES[2].store((*api).TimeType as usize, Ordering::Relaxed);
    }
    capture_sp(cx)?;
    Ok(())
}
