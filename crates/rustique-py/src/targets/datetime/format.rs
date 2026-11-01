//! `date.strftime` (also `datetime.strftime`, `format(dt, spec)`, f-strings), the
//! `datetime.time.strftime` method and the function `time.strftime`.
//!
//! The `datetime` methods call `self.timetuple()` (which builds a `time.struct_time` and, for
//! an aware `datetime`, calls `tzinfo.dst(self)`), import `time.strftime`, rewrite `%z`,
//! `%:z`, `%Z`, `%f` (and the year for years below 1000) in the format, calling
//! `tzinfo.utcoffset(self)`/`tzinfo.tzname(self)` at their first occurrence, and hand the
//! result to glibc's `wcsftime`. The kernels produce the same text in one pass when:
//! - the receiver is an exact `datetime`/`date`/`time`; the format an exact ASCII `str`;
//! - its tzinfo is `None`, an exact `datetime.timezone` (fields read directly: its methods
//!   are pure C) or an exact `zoneinfo.ZoneInfo` (its C methods are called in CPython's
//!   order; they are pure, so any unexpected result restarts in the original);
//! - the directives are in `rustique_core::datetime::strftime`'s subset, names only with
//!   `LC_TIME` = C and no per-thread locale;
//! - `PyImport_Import` would resolve `time` to the validated module, and `time.strftime`
//!   and `time.struct_time` are the originals (a monkeypatched `time.strftime` is honoured
//!   by falling back).

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyString};

use rustique_core::datetime::cal;
use rustique_core::datetime::strftime::{self, Compiled, Needs, Out, Tm, TzText};

use super::env::{self, Env, item, obj};
use super::{capi, fmtcache, ustr, watch};
use crate::accel::{Counters, Original, Reason, check};

type O = *mut ffi::PyObject;
pub type FastKw = unsafe extern "C" fn(O, *const O, ffi::Py_ssize_t, O) -> O;
pub type VarKw = unsafe extern "C" fn(O, O, O) -> O;
pub type VarArgs = unsafe extern "C" fn(O, O) -> O;

pub static C_DATE: Counters = Counters::new();
pub static C_TIME: Counters = Counters::new();
pub static C_FN: Counters = Counters::new();
pub static ORIG_DATE_FAST: Original<FastKw> = Original::new();
pub static ORIG_DATE_VAR: Original<VarKw> = Original::new();
pub static ORIG_TIME_FAST: Original<FastKw> = Original::new();
pub static ORIG_TIME_VAR: Original<VarKw> = Original::new();
pub static ORIG_FN: Original<VarArgs> = Original::new();

/// `datetime.timezone` layout (private struct: `offset`, `name` after the header),
/// calibrated at install.
static TZ_TYPE: AtomicUsize = AtomicUsize::new(0);
static TZ_LAYOUT_OK: AtomicUsize = AtomicUsize::new(0);
/// The main interpreter's `zoneinfo.ZoneInfo` (C), captured when first seen imported.
static ZI_TYPE: AtomicUsize = AtomicUsize::new(0);

static FT_OK: AtomicU64 = AtomicU64::new(0);
static FT_BAD: AtomicU64 = AtomicU64::new(0);

#[repr(C)]
struct RawTimeZone {
    ob_base: ffi::PyObject,
    offset: O,
    name: O,
}

/// Check the `timezone` layout on objects we create: `timezone(timedelta(hours=1), "X")`.
pub fn calibrate_timezone(py: Python<'_>) -> PyResult<bool> {
    // SAFETY: datetime C API filled; attached thread.
    unsafe {
        let api = capi();
        let delta = ((*api).Delta_FromDelta)(0, 3600, 0, 1, (*api).DeltaType);
        let delta = Bound::from_owned_ptr_or_err(py, delta)?;
        let name = PyString::new(py, "rustique-probe");
        let tz = ((*api).TimeZone_FromTimeZone)(delta.as_ptr(), name.as_ptr());
        let tz = Bound::from_owned_ptr_or_err(py, tz)?;
        let raw = &*tz.as_ptr().cast::<RawTimeZone>();
        let utc = &*(*api).TimeZone_UTC.cast::<RawTimeZone>();
        let ok = raw.offset == delta.as_ptr()
            && raw.name == name.as_ptr()
            && utc.name.is_null()
            && ffi::Py_TYPE(utc.offset) == (*api).DeltaType
            && ffi::Py_TYPE(tz.as_ptr()) == ffi::Py_TYPE((*api).TimeZone_UTC)
            && tz.getattr("utcoffset")?.call1((py.None(),))?.is(&delta);
        TZ_TYPE.store(ffi::Py_TYPE((*api).TimeZone_UTC) as usize, Ordering::Relaxed);
        TZ_LAYOUT_OK.store(usize::from(ok), Ordering::Relaxed);
        Ok(ok)
    }
}

// ---------------------------------------------------------------------------------------
// Steady state of the dependencies
// ---------------------------------------------------------------------------------------

fn capture_zoneinfo(py: Python<'_>, e: &Env) {
    if ZI_TYPE.load(Ordering::Relaxed) != 0 {
        return;
    }
    // SAFETY: captured dict.
    let Ok(modules) = (unsafe { obj(py, e.sys_modules) }).cast_into::<PyDict>() else { return };
    let (Some(zi), Some(czi)) = (item(&modules, "zoneinfo"), item(&modules, "_zoneinfo")) else { return };
    let (Ok(zi), Ok(czi)) = (zi.cast_into::<pyo3::types::PyModule>(), czi.cast_into::<pyo3::types::PyModule>()) else {
        return;
    };
    let (Some(t), Some(ct)) = (item(&zi.dict(), "ZoneInfo"), item(&czi.dict(), "ZoneInfo")) else { return };
    if !t.is(&ct) {
        return; // the pure-Python fallback
    }
    // SAFETY: type-checked reads of a type object.
    unsafe {
        let p = t.as_ptr();
        if ffi::PyType_Check(p) == 0 {
            return;
        }
        let ty = p.cast::<ffi::PyTypeObject>();
        let immutable = ffi::PyType_GetFlags(ty) & ffi::Py_TPFLAGS_IMMUTABLETYPE != 0;
        let name_ok = std::ffi::CStr::from_ptr((*ty).tp_name).to_bytes() == b"zoneinfo.ZoneInfo";
        if immutable && name_ok && (*ty).tp_dictoffset == 0 {
            ZI_TYPE.store(t.into_ptr() as usize, Ordering::Relaxed);
        }
    }
}

fn validate(py: Python<'_>, e: &Env) -> bool {
    capture_zoneinfo(py, e);
    env::common_ok(py, e)
}

#[inline(always)]
fn ready(py: Python<'_>, e: &Env) -> bool {
    let g = watch::current();
    if g == FT_OK.load(Ordering::Relaxed) {
        return true;
    }
    if g == FT_BAD.load(Ordering::Relaxed) {
        return false;
    }
    revalidate(py, e, g)
}

#[cold]
#[inline(never)]
fn revalidate(py: Python<'_>, e: &Env, g: u64) -> bool {
    let ok = validate(py, e);
    if ok {
        FT_OK.store(g, Ordering::Relaxed);
    } else {
        FT_BAD.store(g, Ordering::Relaxed);
    }
    ok
}

// ---------------------------------------------------------------------------------------
// tzinfo
// ---------------------------------------------------------------------------------------

/// Replacement texts, computed in `wrap_strftime`'s order. `None`: fall back (nothing
/// observable happened: only pure C methods were called).
struct Tz {
    z: Vec<u8>,
    colon_z: Vec<u8>,
    name: Vec<u8>,
}

/// `(days, seconds, microseconds)` of a valid utcoffset/dst result, `None` = Python None;
/// `Err` = CPython would raise (or not an exact timedelta: let CPython decide).
unsafe fn offset_fields(o: O) -> Result<Option<(i32, i32, i32)>, ()> {
    // SAFETY: live object from a call; datetime C API filled.
    unsafe {
        if o == ffi::Py_None() {
            return Ok(None);
        }
        if ffi::Py_TYPE(o) != (*capi()).DeltaType {
            return Err(());
        }
        let d = o.cast::<ffi::PyDateTime_Delta>();
        let (days, secs, us) = ((*d).days, (*d).seconds, (*d).microseconds);
        if (days == -1 && secs == 0 && us < 1) || !(-1..1).contains(&days) {
            return Err(());
        }
        Ok(Some((days, secs, us)))
    }
}

fn offset_text(f: Option<(i32, i32, i32)>, sep: &[u8]) -> Vec<u8> {
    let mut v = Vec::new();
    if let Some((d, s, u)) = f {
        strftime::utcoffset_text(d, s, u, sep, &mut v);
    }
    v
}

/// Call `tz.<name>(arg)` (a pure C method of an exact `ZoneInfo`).
unsafe fn call_pure(tz: O, name: &std::ffi::CStr, arg: O) -> Option<O> {
    // SAFETY: attached thread; live objects.
    unsafe {
        let n = ffi::PyUnicode_InternFromString(name.as_ptr());
        if n.is_null() {
            ffi::PyErr_Clear();
            return None;
        }
        let r = ffi::PyObject_CallMethodOneArg(tz, n, arg);
        ffi::Py_DECREF(n);
        if r.is_null() {
            ffi::PyErr_Clear();
            return None;
        }
        Some(r)
    }
}

/// The replacements for `tzinfo` (`arg` is what CPython passes: the datetime, or None for
/// `time` objects), or `None` to fall back.
unsafe fn tz_texts(tzinfo: O, arg: O, needs: &Needs, is_datetime: bool) -> Option<Tz> {
    let mut out = Tz { z: Vec::new(), colon_z: Vec::new(), name: Vec::new() };
    // SAFETY: attached thread; live objects; datetime C API filled.
    unsafe {
        if tzinfo == ffi::Py_None() {
            return Some(out);
        }
        let t = ffi::Py_TYPE(tzinfo) as usize;
        if t == TZ_TYPE.load(Ordering::Relaxed) && TZ_LAYOUT_OK.load(Ordering::Relaxed) == 1 {
            // timezone: dst() is None; utcoffset() is `offset`; tzname() is `name` or the
            // "UTC±HH:MM" text (timezone_str).
            let raw = &*tzinfo.cast::<RawTimeZone>();
            let f = offset_fields(raw.offset).ok()?;
            for &c in needs.tz_order.iter().filter(|&&c| c != 0) {
                match c {
                    b'z' => out.z = offset_text(f, b""),
                    b':' => out.colon_z = offset_text(f, b":"),
                    _ => {
                        if raw.name.is_null() {
                            match f {
                                Some((0, 0, 0)) | None => out.name = b"UTC".to_vec(),
                                Some(_) => {
                                    out.name = b"UTC".to_vec();
                                    out.name.extend(offset_text(f, b":"));
                                }
                            }
                        } else {
                            out.name =
                                ustr::ascii(raw.name).filter(|_| ffi::PyUnicode_CheckExact(raw.name) != 0)?.to_vec();
                        }
                    }
                }
            }
            return Some(out);
        }
        if is_datetime && t != 0 && t == zoneinfo_type() {
            // timetuple(): dst(self) first (its value only feeds tm_isdst, unused here).
            let dst = call_pure(tzinfo, c"dst", arg)?;
            let ok = offset_fields(dst).is_ok();
            ffi::Py_DECREF(dst);
            if !ok {
                return None;
            }
            for &c in needs.tz_order.iter().filter(|&&c| c != 0) {
                match c {
                    b'z' | b':' => {
                        let r = call_pure(tzinfo, c"utcoffset", arg)?;
                        let f = offset_fields(r);
                        ffi::Py_DECREF(r);
                        let f = f.ok()?;
                        if c == b'z' {
                            out.z = offset_text(f, b"");
                        } else {
                            out.colon_z = offset_text(f, b":");
                        }
                    }
                    _ => {
                        let r = call_pure(tzinfo, c"tzname", arg)?;
                        let name = if r == ffi::Py_None() {
                            Some(Vec::new())
                        } else if ffi::PyUnicode_CheckExact(r) != 0 {
                            ustr::ascii(r).map(<[u8]>::to_vec)
                        } else {
                            None
                        };
                        ffi::Py_DECREF(r);
                        out.name = name?;
                    }
                }
            }
            return Some(out);
        }
        None
    }
}

/// `format_utcoffset(..., ":", tzinfo, arg)` as `isoformat` appends it: exact `timezone`
/// (fields read directly), or exact `ZoneInfo` for datetimes (its pure C `utcoffset`).
/// `None`: fall back.
///
/// # Safety
/// Attached thread; live objects.
pub(super) unsafe fn utcoffset_colon(tzinfo: O, arg: O, is_datetime: bool) -> Option<Vec<u8>> {
    // SAFETY: per the caller.
    unsafe {
        let t = ffi::Py_TYPE(tzinfo) as usize;
        if t == TZ_TYPE.load(Ordering::Relaxed) && TZ_LAYOUT_OK.load(Ordering::Relaxed) == 1 {
            let raw = &*tzinfo.cast::<RawTimeZone>();
            return Some(offset_text(offset_fields(raw.offset).ok()?, b":"));
        }
        if is_datetime && t != 0 && t == zoneinfo_type() {
            let r = call_pure(tzinfo, c"utcoffset", arg)?;
            let f = offset_fields(r);
            ffi::Py_DECREF(r);
            return Some(offset_text(f.ok()?, b":"));
        }
        None
    }
}

/// The captured `ZoneInfo` type (0 until seen imported; looked up at most once per
/// dependency generation).
fn zoneinfo_type() -> usize {
    let t = ZI_TYPE.load(Ordering::Relaxed);
    if t != 0 {
        return t;
    }
    static TRIED: AtomicU64 = AtomicU64::new(0);
    let g = watch::current();
    if TRIED.swap(g, Ordering::Relaxed) == g {
        return 0;
    }
    if let Some(e) = env::env() {
        // SAFETY: called from kernels (attached thread).
        capture_zoneinfo(unsafe { Python::assume_attached() }, e);
    }
    ZI_TYPE.load(Ordering::Relaxed)
}

// ---------------------------------------------------------------------------------------
// Kernels
// ---------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Recv {
    DateTime,
    Date,
    Time,
}

static WRAPPED: fmtcache::Table<Compiled> = fmtcache::Table::new();
static PLAIN: fmtcache::Table<Compiled> = fmtcache::Table::new();
thread_local! {
    static LOCAL_WRAPPED: fmtcache::Local<Compiled> = const { fmtcache::Local::new() };
    static LOCAL_PLAIN: fmtcache::Local<Compiled> = const { fmtcache::Local::new() };
}

/// The compiled format (`None`: not ASCII or not in the subset).
///
/// # Safety
/// Attached thread; `fmt` a live exact `str`.
#[inline(always)]
unsafe fn compiled(fmt: O, wrapped: bool) -> Option<fmtcache::Cached<Compiled>> {
    // SAFETY: per the caller.
    unsafe {
        if wrapped {
            fmtcache::lookup(&WRAPPED, &LOCAL_WRAPPED, fmt, |fb| strftime::compile(fb, true))
        } else {
            fmtcache::lookup(&PLAIN, &LOCAL_PLAIN, fmt, |fb| strftime::compile(fb, false))
        }
    }
}

#[inline]
unsafe fn result(buf: &[u8]) -> O {
    // SAFETY: ASCII bytes.
    unsafe { ffi::PyUnicode_FromStringAndSize(buf.as_ptr().cast(), buf.len() as ffi::Py_ssize_t) }
}

fn names_ok(needs: &Needs) -> bool {
    !needs.names || (env::lc_time_is_c() && env::thread_locale_is_global())
}

/// `date.strftime`/`time.strftime` methods: `None` = fall back (nothing observable done).
unsafe fn method(this: O, fmt: O, time_method: bool) -> Option<O> {
    let c = if time_method { &C_TIME } else { &C_DATE };
    // SAFETY: attached thread; live objects; datetime C API filled at install.
    unsafe {
        let api = capi();
        let t = ffi::Py_TYPE(this);
        let recv = if time_method {
            if t != (*api).TimeType {
                c.fallback(Reason::Args);
                return None;
            }
            Recv::Time
        } else if t == (*api).DateTimeType {
            Recv::DateTime
        } else if t == (*api).DateType {
            Recv::Date
        } else {
            c.fallback(Reason::Args);
            return None;
        };
        if ffi::PyUnicode_CheckExact(fmt) == 0 {
            c.fallback(Reason::Args);
            return None;
        }
        let Some(cf) = compiled(fmt, true).filter(|cf| names_ok(&cf.needs)) else {
            c.fallback(Reason::Args);
            return None;
        };
        let needs = cf.needs;
        let e = env::env()?;
        if !env::frame_ok(e) {
            c.fallback(Reason::Interp);
            return None;
        }
        let py = Python::assume_attached();
        if !ready(py, e) {
            c.fallback(Reason::Deps);
            return None;
        }
        let (tm, micro, tzinfo) = match recv {
            Recv::DateTime | Recv::Date => {
                let (y, m, d) = (
                    ffi::PyDateTime_GET_YEAR(this),
                    ffi::PyDateTime_GET_MONTH(this) as u32,
                    ffi::PyDateTime_GET_DAY(this) as u32,
                );
                let year = i64::from(y);
                let (hh, mm, ss, us, tz) = if recv == Recv::DateTime {
                    (
                        ffi::PyDateTime_DATE_GET_HOUR(this) as u32,
                        ffi::PyDateTime_DATE_GET_MINUTE(this) as u32,
                        ffi::PyDateTime_DATE_GET_SECOND(this) as u32,
                        ffi::PyDateTime_DATE_GET_MICROSECOND(this) as u32,
                        ffi::PyDateTime_DATE_GET_TZINFO(this),
                    )
                } else {
                    (0, 0, 0, 0, ffi::Py_None())
                };
                let tm = Tm {
                    year,
                    month: m,
                    mday: d,
                    hour: hh,
                    minute: mm,
                    second: ss,
                    wday: (cal::weekday(year, m, d) + 1) % 7,
                    yday: cal::day_of_year(year, m, d),
                };
                (tm, us, tz)
            }
            Recv::Time => {
                let tm = Tm {
                    year: 1900,
                    month: 1,
                    mday: 1,
                    hour: ffi::PyDateTime_TIME_GET_HOUR(this) as u32,
                    minute: ffi::PyDateTime_TIME_GET_MINUTE(this) as u32,
                    second: ffi::PyDateTime_TIME_GET_SECOND(this) as u32,
                    wday: 1,
                    yday: 1,
                };
                (tm, ffi::PyDateTime_TIME_GET_MICROSECOND(this) as u32, ffi::PyDateTime_TIME_GET_TZINFO(this))
            }
        };
        if needs.plain_year && tm.year < 1000 {
            c.fallback(Reason::Args);
            return None;
        }
        let arg = if recv == Recv::Time { ffi::Py_None() } else { this };
        let Some(tz) = tz_texts(tzinfo, arg, &needs, recv == Recv::DateTime) else {
            c.fallback(Reason::Args);
            return None;
        };
        let mut out = Out::new();
        strftime::render_compiled(
            &cf,
            &tm,
            micro,
            &TzText { z: &tz.z, colon_z: &tz.colon_z, name: &tz.name },
            &mut out,
        );
        let Some(bytes) = out.bytes() else {
            c.fallback(Reason::Args);
            return None;
        };
        Some(result(bytes))
    }
}

macro_rules! method_kernels {
    ($fast:ident, $var:ident, $time:expr, $c:expr, $of:expr, $ov:expr, $spec:expr) => {
        /// METH_FASTCALL | METH_KEYWORDS (3.15).
        ///
        /// # Safety
        /// Called by CPython with this calling convention's arguments.
        pub unsafe extern "C" fn $fast(this: O, args: *const O, nargs: ffi::Py_ssize_t, kwnames: O) -> O {
            // SAFETY: CPython's calling convention; fallback with unchanged arguments.
            unsafe {
                if nargs == 1 && (kwnames.is_null() || ffi::PyTuple_GET_SIZE(kwnames) == 0) {
                    if let Some(r) = method(this, *args, $time) {
                        if $c.hit_check() {
                            return check::verify(&$spec, r, &[this, *args], || $of.get()(this, args, nargs, kwnames));
                        }
                        return r;
                    }
                } else {
                    $c.fallback(Reason::Args);
                }
                $of.get()(this, args, nargs, kwnames)
            }
        }

        /// METH_VARARGS | METH_KEYWORDS (3.14).
        ///
        /// # Safety
        /// Called by CPython with this calling convention's arguments.
        pub unsafe extern "C" fn $var(this: O, args: O, kw: O) -> O {
            // SAFETY: CPython's calling convention; fallback with unchanged arguments.
            unsafe {
                if ffi::PyTuple_GET_SIZE(args) == 1 && (kw.is_null() || ffi::PyDict_Size(kw) == 0) {
                    let f = ffi::PyTuple_GET_ITEM(args, 0);
                    if let Some(r) = method(this, f, $time) {
                        if $c.hit_check() {
                            return check::verify(&$spec, r, &[this, f], || $ov.get()(this, args, kw));
                        }
                        return r;
                    }
                } else {
                    $c.fallback(Reason::Args);
                }
                $ov.get()(this, args, kw)
            }
        }
    };
}

method_kernels!(date_fast, date_var, false, C_DATE, ORIG_DATE_FAST, ORIG_DATE_VAR, super::DATE_STRFTIME);
method_kernels!(time_fast, time_var, true, C_TIME, ORIG_TIME_FAST, ORIG_TIME_VAR, super::TIME_STRFTIME);

/// An exact int that fits a C int (compact ints read inline).
#[inline(always)]
unsafe fn small_int(o: O) -> Option<i32> {
    // SAFETY: attached thread; live object.
    unsafe {
        if ffi::PyLong_CheckExact(o) == 0 {
            return None;
        }
        crate::convert::exact_int_i64(o).and_then(|v| i32::try_from(v).ok())
    }
}

/// `time.strftime(format, tuple)` with an exact 9-tuple of ints or an exact `struct_time`.
unsafe fn function(module: O, args: O) -> Option<O> {
    // SAFETY: attached thread; live objects.
    unsafe {
        let e = env::env()?;
        if module as usize != e.time_mod || ffi::PyTuple_GET_SIZE(args) != 2 {
            return None;
        }
        let fmt = ffi::PyTuple_GET_ITEM(args, 0);
        let tup = ffi::PyTuple_GET_ITEM(args, 1);
        if ffi::PyUnicode_CheckExact(fmt) == 0 {
            return None;
        }
        // struct_time: 9 visible items plus tm_zone and tm_gmtoff stored after them.
        let is_st = ffi::Py_TYPE(tup) as usize == e.struct_time;
        if !(ffi::PyTuple_CheckExact(tup) != 0 || is_st) || ffi::PyTuple_GET_SIZE(tup) != 9 {
            return None;
        }
        let mut v = [0i32; 9];
        for (i, x) in v.iter_mut().enumerate() {
            *x = small_int(ffi::PyTuple_GET_ITEM(tup, i as ffi::Py_ssize_t))?;
        }
        let [y, mon, mday, hour, min, sec, wday, yday, _isdst] = v;
        if !((1000..=9999).contains(&y)
            && (1..=12).contains(&mon)
            && (1..=31).contains(&mday)
            && (0..=23).contains(&hour)
            && (0..=59).contains(&min)
            && (0..=61).contains(&sec)
            && (0..=6).contains(&wday)
            && (1..=366).contains(&yday))
        {
            return None;
        }
        let cf = compiled(fmt, false).filter(|cf| names_ok(&cf.needs))?;
        if is_st {
            // gettmarg also reads tm_zone (PyUnicode_AsUTF8) and tm_gmtoff (PyLong_AsLong).
            let zone = ffi::PyTuple_GET_ITEM(tup, 9);
            let gmtoff = ffi::PyTuple_GET_ITEM(tup, 10);
            let zone_ok = zone == ffi::Py_None() || ffi::PyUnicode_CheckExact(zone) != 0 && ustr::ascii(zone).is_some();
            let off_ok = gmtoff == ffi::Py_None()
                || ffi::PyLong_CheckExact(gmtoff) != 0 && crate::convert::exact_int_i64(gmtoff).is_some();
            if !zone_ok || !off_ok {
                return None;
            }
        }
        let tm = Tm {
            year: i64::from(y),
            month: mon as u32,
            mday: mday as u32,
            hour: hour as u32,
            minute: min as u32,
            second: sec as u32,
            wday: ((wday + 1) % 7) as u32,
            yday: yday as u32,
        };
        let mut out = Out::new();
        strftime::render_compiled(&cf, &tm, 0, &TzText::default(), &mut out);
        Some(result(out.bytes()?))
    }
}

/// `time.strftime` (METH_VARARGS on 3.14 and 3.15).
///
/// # Safety
/// Called by CPython with this calling convention's arguments.
pub unsafe extern "C" fn time_fn(module: O, args: O) -> O {
    // SAFETY: CPython's calling convention; fallback with unchanged arguments.
    unsafe {
        if let Some(r) = function(module, args) {
            if C_FN.hit_check() {
                return check::verify(
                    &super::TIME_FN_STRFTIME,
                    r,
                    &[ffi::PyTuple_GET_ITEM(args, 0), ffi::PyTuple_GET_ITEM(args, 1)],
                    || ORIG_FN.get()(module, args),
                );
            }
            return r;
        }
        C_FN.fallback(Reason::Args);
        ORIG_FN.get()(module, args)
    }
}

/// (ok generation, failed generation).
pub fn validation_state() -> (u64, u64) {
    (FT_OK.load(Ordering::Relaxed), FT_BAD.load(Ordering::Relaxed))
}
