//! `datetime.strptime`, `date.strptime`, `time.strptime` (classmethods of `_datetime`) and
//! the function `time.strptime`: C entry points whose originals import `_strptime` and run
//! its Python code. The kernels parse in Rust (`rustique_core::datetime::strptime`) when the
//! call is in the exact subset and `_strptime` is in a steady state in which its original
//! would neither rebuild its caches nor do anything else observable; otherwise they call
//! the original with the unchanged arguments.

use std::cell::RefCell;
use std::ffi::c_int;
use std::rc::Rc;
use std::sync::atomic::{AtomicPtr, AtomicU64, AtomicUsize, Ordering};

use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList, PyString, PyTuple};

use rustique_core::datetime::strptime::{self, A_MONTH, A_WEEKDAY, AM_PM, F_MONTH, F_WEEKDAY, Parsed, Program, Version};

use super::env::{self, Env, item, obj};
use super::{fingerprints, ustr, watch};
use crate::accel::{Counters, Original, Prepare, Reason, Refusal, Tools, check, code_locally_quiet};

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
    /// `_strptime._regex_cache` and `re.Pattern`.
    regex_cache: usize,
    pattern_type: usize,
    /// (0 = `_strptime`, 1 = `locale`; name; identity of the global or None) at capture.
    shadows: Vec<(u8, &'static str, Option<usize>)>,
}

/// Captured `_strptime` module objects (normally one; `test_datetime` imports fresh copies).
const MAX_SP: usize = 32;
static SPS: [AtomicPtr<SpDeps>; MAX_SP] = [const { AtomicPtr::new(std::ptr::null_mut()) }; MAX_SP];
static SP_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
/// `(generation << 8) | index into SPS` of the last successful validation (0: none).
static SP_OK: AtomicU64 = AtomicU64::new(0);
static SP_BAD: AtomicU64 = AtomicU64::new(0);

fn keep(o: &Bound<'_, PyAny>) -> usize {
    o.clone().into_ptr() as usize
}

/// Dict entry without running Python code.
fn get<'py>(d: &Bound<'py, PyDict>, key: &str) -> Result<Bound<'py, PyAny>, String> {
    item(d, key).ok_or_else(|| format!("missing {key}"))
}

fn fp_ok(name: &str, f: &Bound<'_, PyAny>) -> Result<(), String> {
    let (major, minor) = crate::accel::version::running();
    let observed = crate::accel::fingerprint::function(f).map_err(|e| e.to_string())?;
    match crate::accel::fingerprint::expected_for(fingerprints::code(name), major, minor) {
        Some(x) if x == observed => Ok(()),
        _ => Err(format!("{name}: fingerprint {observed} not validated for {major}.{minor}")),
    }
}

/// Capture one `_strptime` module object: fingerprints of the code the kernels
/// reimplement, the objects its steady state depends on, and generation watches on them.
/// Plain dict reads and C getters only (it may run inside `prepare` or a kernel).
fn capture_module(py: Python<'_>, m: &Bound<'_, pyo3::types::PyModule>) -> Result<SpDeps, String> {
    let pe = |e: PyErr| e.to_string();
    let d = m.dict();
    if crate::accel::prepare::module_initializing(m) {
        return Err("_strptime is still initializing".into());
    }
    let locale = get(&d, "locale")?.cast_into::<pyo3::types::PyModule>().map_err(|e| pe(e.into()))?;
    let ld = locale.dict();
    let mut codes = [(0usize, 0usize); 10];
    for (i, name) in FUNCS.iter().chain(LOCALE_FUNCS.iter()).enumerate() {
        let f = if i < FUNCS.len() { get(&d, name)? } else { get(&ld, name)? };
        if f.cast_exact::<pyo3::types::PyFunction>().is_err() {
            return Err(format!("{name} is not a Python function"));
        }
        fp_ok(name, &f)?;
        let code = f.getattr("__code__").map_err(pe)?;
        if !crate::accel::version::code_layout_ok_for(&code).map_err(pe)? {
            return Err("PyCodeObject layout differs".into());
        }
        codes[i] = (keep(&f), keep(&code));
    }
    fp_ok("_fixmonths", &get(&d, "_fixmonths")?)?;
    for (cls, meth) in CLASS_METHODS {
        let ty = get(&d, cls)?.cast_into::<pyo3::types::PyType>().map_err(|e| pe(e.into()))?;
        let td = crate::accel::prepare::type_dict(&ty).map_err(|r| r.reason().to_string())?;
        fp_ok(&format!("{cls}.{meth}"), &get(&td, meth)?)?;
    }
    // locale._setlocale is the C function _locale.setlocale.
    let setlocale = get(&ld, "_setlocale")?;
    // SAFETY: type-checked reads of a builtin function.
    let c_ok = unsafe {
        let p = setlocale.as_ptr();
        ffi::PyCFunction_Check(p) != 0 && {
            let f = p.cast::<ffi::PyCFunctionObject>();
            let def = (*f).m_ml;
            let owner = (*f).m_self;
            !def.is_null()
                && std::ffi::CStr::from_ptr((*def).ml_name).to_bytes() == b"setlocale"
                && !owner.is_null()
                && ffi::PyModule_Check(owner) != 0
                && Bound::from_borrowed_ptr(py, owner).getattr("__name__").is_ok_and(|n| n.eq("_locale").unwrap_or(false))
        }
    };
    if !c_ok {
        return Err("locale._setlocale is not _locale.setlocale".into());
    }
    let time_strptime = get(&d, "_strptime_time")?;
    let defaults = time_strptime.getattr("__defaults__").map_err(pe)?;
    if !defaults.eq(("%a %b %d %H:%M:%S %Y",)).unwrap_or(false) {
        return Err("_strptime_time default format differs".into());
    }
    let rc = get(&d, "_regex_cache")?;
    if !rc.is_exact_instance_of::<PyDict>() {
        return Err("_strptime._regex_cache is not a dict".into());
    }
    let re = get(&d, "re")?.cast_into::<pyo3::types::PyModule>().map_err(|e| pe(e.into()))?;
    let mut shadows = Vec::new();
    for (which, dict) in [(0u8, &d), (1u8, &ld)] {
        for name in SHADOWS {
            shadows.push((which, name, item(dict, name).map(|o| keep(&o))));
        }
    }
    let alias = get(&ld, "locale_alias")?;
    let sp = SpDeps {
        module: keep(m.as_any()),
        dict: keep(d.as_any()),
        codes,
        time_defaults: keep(&defaults),
        locale_mod: keep(locale.as_any()),
        locale_dict: keep(ld.as_any()),
        locale_alias: keep(&alias),
        setlocale: keep(&setlocale),
        time_re_cls: keep(&get(&d, "TimeRE")?),
        locale_time_cls: keep(&get(&d, "LocaleTime")?),
        cache_lock_type: keep(get(&d, "_cache_lock")?.get_type().as_any()),
        regex_cache: keep(&rc),
        pattern_type: keep(&get(&re.dict(), "Pattern")?),
        shadows,
    };
    let keys_m: Vec<&str> = FUNCS
        .iter()
        .chain(SHADOWS.iter())
        .chain(
            [
                "time",
                "locale",
                "re",
                "datetime_date",
                "datetime_timedelta",
                "datetime_timezone",
                "TimeRE",
                "LocaleTime",
                "_cache_lock",
                "_TimeRE_cache",
                "_regex_cache",
                "_CACHE_MAX_SIZE",
            ]
            .iter(),
        )
        .copied()
        .collect();
    let keys_l: Vec<&str> = LOCALE_FUNCS
        .iter()
        .chain(SHADOWS.iter())
        .chain(["_setlocale", "locale_alias", "LC_TIME", "LC_ALL"].iter())
        .copied()
        .collect();
    watch::watch(&d, &keys_m).map_err(pe)?;
    watch::watch(&ld, &keys_l).map_err(pe)?;
    watch::watch(&alias.cast_into::<PyDict>().map_err(|e| pe(e.into()))?, &["c"]).map_err(pe)?;
    Ok(sp)
}

/// The captured deps of module `m` (capturing it on first sight): (index, deps).
fn find_or_capture(py: Python<'_>, m: &Bound<'_, pyo3::types::PyModule>) -> Result<(usize, &'static SpDeps), String> {
    let find = || {
        (0..MAX_SP).find_map(|i| {
            let p = SPS[i].load(Ordering::Acquire);
            // SAFETY: leaked, never freed.
            (!p.is_null() && unsafe { (*p).module } == m.as_ptr() as usize).then(|| (i, unsafe { &*p }))
        })
    };
    if let Some(hit) = find() {
        return Ok(hit);
    }
    let sp = capture_module(py, m)?;
    let _g = SP_LOCK.lock().map_err(|_| "poisoned".to_string())?;
    if let Some(hit) = find() {
        return Ok(hit);
    }
    let i = (0..MAX_SP).find(|&i| SPS[i].load(Ordering::Acquire).is_null()).ok_or("too many _strptime modules")?;
    let p = Box::into_raw(Box::new(sp));
    SPS[i].store(p, Ordering::Release);
    // SAFETY: just leaked.
    Ok((i, unsafe { &*p }))
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

/// Why the last validation failed (diagnostics: `_accel._datetime_debug()`).
pub static LAST_FAILURE: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

/// Everything the original reads in its steady state is what the kernel assumes.
fn validate(py: Python<'_>, e: &Env) -> Result<usize, String> {
    macro_rules! need {
        ($c:expr, $why:expr) => {
            if !$c {
                return Err($why.to_string());
            }
        };
    }
    let pe = |r: PyErr| format!("error: {}", r.value(py));
    need!(env::common_ok(py, e), "builtins / time dependencies changed");
    // SAFETY: captured objects.
    let (modules, tdict) = unsafe {
        (
            obj(py, e.sys_modules).cast_into::<PyDict>().map_err(|e| pe(e.into()))?,
            obj(py, e.time_dict).cast_into::<PyDict>().map_err(|e| pe(e.into()))?,
        )
    };
    let is = |o: Option<Bound<'_, PyAny>>, addr: usize| o.is_some_and(|o| o.as_ptr() as usize == addr);
    // The `_strptime` that `PyImport_Import` would return now.
    let m = item(&modules, "_strptime").ok_or("_strptime not imported")?;
    let m = m.cast_into::<pyo3::types::PyModule>().map_err(|_| "sys.modules['_strptime'] is not a module")?;
    let (index, sp) = find_or_capture(py, &m)?;
    // SAFETY: captured objects.
    let (d, ldict) = unsafe {
        (
            obj(py, sp.dict).cast_into::<PyDict>().map_err(|e| pe(e.into()))?,
            obj(py, sp.locale_dict).cast_into::<PyDict>().map_err(|e| pe(e.into()))?,
        )
    };
    let int_is = |o: Option<Bound<'_, PyAny>>, v: i64| {
        o.is_some_and(|o| o.is_exact_instance_of::<pyo3::types::PyInt>() && o.eq(v).unwrap_or(false))
    };
    // SAFETY: captured modules.
    let (mt, lt_mod) = unsafe { (ffi::Py_TYPE(sp.module as *mut _) as usize, ffi::Py_TYPE(sp.locale_mod as *mut _) as usize) };
    need!(mt == e.module_type, "_strptime module type changed");
    need!(lt_mod == e.module_type, "locale module type changed");
    for (i, name) in FUNCS.iter().enumerate() {
        need!(is(item(&d, name), sp.codes[i].0), format!("_strptime.{name} changed"));
    }
    for (i, name) in LOCALE_FUNCS.iter().enumerate() {
        need!(is(item(&ldict, name), sp.codes[FUNCS.len() + i].0), format!("locale.{name} changed"));
    }
    let api = super::capi();
    // SAFETY: datetime C API, filled at install.
    let (date_t, delta_t, tz_t) =
        unsafe { ((*api).DateType as usize, (*api).DeltaType as usize, ffi::Py_TYPE((*api).TimeZone_UTC) as usize) };
    need!(is(item(&d, "time"), e.time_mod), "_strptime.time changed");
    need!(is(item(&d, "locale"), sp.locale_mod), "_strptime.locale changed");
    need!(
        is(item(&d, "datetime_date"), date_t)
            && is(item(&d, "datetime_timedelta"), delta_t)
            && is(item(&d, "datetime_timezone"), tz_t),
        "_strptime datetime types changed"
    );
    need!(
        is(item(&d, "TimeRE"), sp.time_re_cls) && is(item(&d, "LocaleTime"), sp.locale_time_cls),
        "_strptime classes changed"
    );
    need!(
        item(&d, "_cache_lock").is_some_and(|l| l.get_type().as_ptr() as usize == sp.cache_lock_type),
        "_strptime._cache_lock changed"
    );
    need!(
        is(item(&d, "_regex_cache"), sp.regex_cache) && int_is(item(&d, "_CACHE_MAX_SIZE"), 5),
        "_strptime._regex_cache / _CACHE_MAX_SIZE changed"
    );
    need!(
        is(item(&ldict, "_setlocale"), sp.setlocale) && is(item(&ldict, "locale_alias"), sp.locale_alias),
        "locale._setlocale / locale_alias changed"
    );
    need!(int_is(item(&ldict, "LC_TIME"), 2) && int_is(item(&ldict, "LC_ALL"), 6), "locale.LC_* changed");
    for &(which, name, expected) in &sp.shadows {
        let dict = if which == 0 { &d } else { &ldict };
        need!(item(dict, name).map(|o| o.as_ptr() as usize) == expected, format!("global {name} shadowing changed"));
    }
    // SAFETY: captured dict.
    let alias = unsafe { obj(py, sp.locale_alias) }.cast_into::<PyDict>().map_err(|e| pe(e.into()))?;
    need!(
        item(&alias, "c").is_some_and(|v| v.is_exact_instance_of::<PyString>() && v.eq("C").unwrap_or(false)),
        "locale_alias['c'] changed"
    );
    need!(int_is(item(&tdict, "_STRUCT_TM_ITEMS"), 11), "time._STRUCT_TM_ITEMS changed");
    // The cached LocaleTime: C locale content, and the original's recreation test
    // (`_getlang() != lang or time.tzname != tzname or time.daylight != daylight`) is false
    // while LC_TIME is "C" (checked per call).
    let Some(cache) = item(&d, "_TimeRE_cache") else { return Err("no _TimeRE_cache".into()) };
    need!(cache.get_type().as_ptr() as usize == sp.time_re_cls, "_TimeRE_cache type");
    let lt = cache.getattr("locale_time").map_err(pe)?;
    need!(lt.get_type().as_ptr() as usize == sp.locale_time_cls, "locale_time type");
    let lang = lt.getattr("lang").map_err(pe)?;
    need!(
        lang.is_exact_instance_of::<PyTuple>() && lang.eq((py.None(), py.None())).unwrap_or(false),
        "cached LocaleTime is not for the C locale"
    );
    let (Some(tzname), Some(daylight)) = (item(&tdict, "tzname"), item(&tdict, "daylight")) else {
        return Err("no time.tzname/daylight".into());
    };
    let lt_tz = lt.getattr("tzname").map_err(pe)?;
    let lt_dl = lt.getattr("daylight").map_err(pe)?;
    need!(
        env::is_str_tuple(&tzname) && env::is_str_tuple(&lt_tz) && tzname.eq(&lt_tz).unwrap_or(false),
        "time.tzname differs from the cached one"
    );
    let int_t = py.get_type::<pyo3::types::PyInt>();
    need!(
        daylight.is_exact_instance(&int_t) && lt_dl.is_exact_instance(&int_t) && daylight.eq(&lt_dl).unwrap_or(false),
        "time.daylight differs from the cached one"
    );
    let alt = lt.getattr("LC_alt_digits").map_err(pe)?;
    need!(alt.is_exact_instance_of::<PyTuple>() && alt.len().unwrap_or(1) == 0, "LC_alt_digits");
    let names = |a: &str, list: &[&str], lead| lt.getattr(a).is_ok_and(|o| list_is(&o, list, lead));
    need!(
        names("a_weekday", &A_WEEKDAY, false)
            && names("f_weekday", &F_WEEKDAY, false)
            && names("a_month", &A_MONTH, true)
            && names("f_month", &F_MONTH, true)
            && names("am_pm", &AM_PM, false),
        "cached LocaleTime names are not the C locale's"
    );
    Ok(index)
}

#[inline(always)]
fn sp_at(i: usize) -> Option<&'static SpDeps> {
    let p = SPS[i].load(Ordering::Acquire);
    // SAFETY: leaked, never freed.
    (!p.is_null()).then(|| unsafe { &*p })
}

/// The deps of `sys.modules['_strptime']` if it is in the validated steady state. One
/// relaxed load in the common case.
#[inline(always)]
fn ready(py: Python<'_>, e: &Env) -> Option<&'static SpDeps> {
    let g = watch::current();
    let ok = SP_OK.load(Ordering::Acquire);
    if ok >> 8 == g {
        return sp_at((ok & 0xff) as usize);
    }
    if g == SP_BAD.load(Ordering::Relaxed) {
        return None;
    }
    revalidate(py, e, g)
}

#[cold]
#[inline(never)]
fn revalidate(py: Python<'_>, e: &Env, g: u64) -> Option<&'static SpDeps> {
    match validate(py, e) {
        Ok(i) => {
            SP_OK.store(g << 8 | i as u64, Ordering::Release);
            sp_at(i)
        }
        Err(why) => {
            SP_BAD.store(g, Ordering::Relaxed);
            if let Ok(mut last) = LAST_FAILURE.lock() {
                *last = why;
            }
            None
        }
    }
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

/// `len(_regex_cache) <= 5` and `_regex_cache.get(fmt)` is a compiled pattern.
///
/// # Safety
/// Attached thread; `fmt` an exact str.
#[inline(always)]
unsafe fn regex_cached(sp: &SpDeps, fmt: *mut ffi::PyObject) -> bool {
    // SAFETY: the captured dict (kept alive); exact str key: no Python code runs.
    unsafe {
        let rc = sp.regex_cache as *mut ffi::PyObject;
        if ffi::PyDict_Size(rc) > 5 {
            return false;
        }
        let mut v: *mut ffi::PyObject = std::ptr::null_mut();
        let r = ffi::PyDict_GetItemRef(rc, fmt, &mut v);
        if r <= 0 {
            if r < 0 {
                ffi::PyErr_Clear();
            }
            return false;
        }
        let ok = ffi::Py_TYPE(v) as usize == sp.pattern_type;
        ffi::Py_DECREF(v);
        ok
    }
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
        if ffi::PyUnicode_CheckExact(s) == 0 || !fmt.is_null() && ffi::PyUnicode_CheckExact(fmt) == 0 {
            c.fallback(Reason::Args);
            return None;
        }
        let Some(e) = env::env() else { return None };
        if !env::frame_ok(e) {
            c.fallback(Reason::Interp);
            return None;
        }
        let py = Python::assume_attached();
        let Some(sp) = ready(py, e).filter(|_| env::lc_time_is_c()) else {
            c.fallback(Reason::Deps);
            return None;
        };
        if !TOOLS.quiet() || !codes_ok(sp, kind, false) {
            c.fallback(Reason::Tools);
            return None;
        }
        // `time.strptime(s)`: `_strptime_time`'s default format, from its (unchanged)
        // `__defaults__`.
        let fmt = if fmt.is_null() {
            let f = sp.codes[Kind::TimeFn as usize].0 as *mut ffi::PyFunctionObject;
            let d = (*f).func_defaults;
            if d as usize != sp.time_defaults {
                c.fallback(Reason::Deps);
                return None;
            }
            ffi::PyTuple_GET_ITEM(d, 0)
        } else {
            fmt
        };
        // The original's private cache: it clears `_regex_cache` when it holds more than
        // `_CACHE_MAX_SIZE` (5) entries and compiles (and may warn) on a miss. Take the fast
        // path only when it would do neither, so the cache evolves exactly as in CPython.
        if !regex_cached(sp, fmt) {
            c.fallback(Reason::Deopt);
            return None;
        }
        let (Some(fb), Some(sb)) = (ustr::ascii(fmt), ustr::ascii(s)) else {
            c.fallback(Reason::Args);
            return None;
        };
        let Some(p) = program(fb).and_then(|prog| prog.parse(sb, kind == Kind::TimeFn)) else {
            c.fallback(Reason::Args);
            return None;
        };
        let r = build(kind, &p, sp);
        if r.is_none() {
            c.fallback(Reason::Args);
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
                        if COUNTERS[$kind as usize].hit_check() {
                            return check::verify(super::spec(&$kind), r, &[*args, *args.add(1)], || {
                                ORIG_FAST[$kind as usize].get()(cls, args, nargs)
                            });
                        }
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
                    let (a, b) = (ffi::PyTuple_GET_ITEM(args, 0), ffi::PyTuple_GET_ITEM(args, 1));
                    if let Some(r) = run($kind, cls, a, b) {
                        if COUNTERS[$kind as usize].hit_check() {
                            return check::verify(super::spec(&$kind), r, &[a, b], || {
                                ORIG_VAR[$kind as usize].get()(cls, args)
                            });
                        }
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
            // NULL: the default format.
            let fmt = if n == 2 { ffi::PyTuple_GET_ITEM(args, 1) } else { std::ptr::null_mut() };
            if let Some(r) = run(Kind::TimeFn, module, s, fmt) {
                if COUNTERS[Kind::TimeFn as usize].hit_check() {
                    let a: &[*mut ffi::PyObject] = if n == 2 { &[s, fmt] } else { &[s] };
                    return check::verify(super::spec(&Kind::TimeFn), r, a, || {
                        ORIG_VAR[Kind::TimeFn as usize].get()(module, args)
                    });
                }
                return r;
            }
        } else {
            COUNTERS[Kind::TimeFn as usize].fallback(Reason::Args);
        }
        ORIG_VAR[Kind::TimeFn as usize].get()(module, args)
    }
}

/// Setup shared by the strptime targets. `_strptime` itself is captured (fingerprinted,
/// watched) when the kernels first see it in `sys.modules`; when it is already imported,
/// its fingerprints are checked here too, so a mismatch shows as a refusal in `status()`.
pub fn prepare_common(cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    let (_, minor) = cx.version();
    VERSION.store(usize::from(minor), Ordering::Relaxed);
    let api = super::capi();
    // SAFETY: datetime C API filled by the caller.
    let date_t = unsafe {
        TYPES[0].store((*api).DateTimeType as usize, Ordering::Relaxed);
        TYPES[1].store((*api).DateType as usize, Ordering::Relaxed);
        TYPES[2].store((*api).TimeType as usize, Ordering::Relaxed);
        (*api).DateType as usize
    };
    let Ok(m) = cx.loaded_module("_strptime") else { return Ok(()) };
    let d = m.dict();
    // Only the stdlib's C-datetime flavour (not one imported against `_pydatetime`).
    if !cx.get_opt(&d, "datetime_date")?.is_some_and(|t| t.as_ptr() as usize == date_t) {
        return Ok(());
    }
    let locale = cx.get(&d, "locale")?.cast_into::<pyo3::types::PyModule>().map_err(PyErr::from)?;
    for name in FUNCS {
        cx.fingerprint_code(&cx.get(&d, name)?, fingerprints::code(name))?;
    }
    for name in LOCALE_FUNCS {
        cx.fingerprint_code(&cx.get(&locale.dict(), name)?, fingerprints::code(name))?;
    }
    cx.fingerprint_code(&cx.get(&d, "_fixmonths")?, fingerprints::code("_fixmonths"))?;
    for (cls, meth) in CLASS_METHODS {
        let ty = cx.get(&d, cls)?.cast_into::<pyo3::types::PyType>().map_err(PyErr::from)?;
        cx.fingerprint_code(&cx.type_attr(&ty, meth)?, fingerprints::code(&format!("{cls}.{meth}")))?;
    }
    Ok(())
}

/// (ok generation, failed generation, last failure reason).
pub fn validation_state() -> (u64, u64, String) {
    (
        SP_OK.load(Ordering::Relaxed) >> 8,
        SP_BAD.load(Ordering::Relaxed),
        LAST_FAILURE.lock().map(|s| s.clone()).unwrap_or_default(),
    )
}
