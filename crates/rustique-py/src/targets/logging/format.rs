//! `Formatter.formatTime` and `Formatter.format` (targets `logging.Formatter.formatTime`,
//! `logging.Formatter.format`).
//!
//! `formatTime`: `self.converter(record.created)` is `time.localtime` or `time.gmtime`
//! (the C functions, by identity), so the broken-down time comes from the same
//! `localtime_r`/`gmtime_r` call CPython makes; `time.strftime(fmt, ct)` is produced
//! natively for formats made of the numeric directives `%Y %m %d %H %M %S %y %j %%`
//! (locale-independent), otherwise by calling `time.localtime`/`time.strftime` themselves;
//! `default_msec_format % (s, record.msecs)` natively for the default `'%s,%03d'`,
//! otherwise with `PyUnicode_Format`.
//!
//! `format`: `record.getMessage()` (str `msg`, arguments of plain types), `usesTime()`,
//! `formatTime()`, `formatMessage()` → `PercentStyle.format` → `self._fmt % values`, for
//! `PercentStyle` formatters and records without `exc_info`/`exc_text`/`stack_info`.
//! Every value the `%`-format reads is a plain `str`/`int`/`float`/`bool`/`None`, so no
//! Python code runs below the kernel; any formatting error re-runs CPython's original
//! from the start (the original's frames then produce the traceback `handleError` prints).

use pyo3::ffi;

use super::common::*;
use super::record::RECORDS;
use crate::accel::guard::DepFlag;
use crate::accel::stats::{Counters, Reason};
use rustique_core::logging::{Tm, msec_suffix, percent_keys, strftime_numeric};

pub static TIME_DEPS: DepFlag = DepFlag::new();
pub static TIME_STATS: Counters = Counters::new();
pub static FORMAT_DEPS: DepFlag = DepFlag::new();
pub static FORMAT_STATS: Counters = Counters::new();

/// `time.localtime`, `time.gmtime`, `time.strftime` (the C functions).
pub static LOCALTIME: Cap = Cap::new();
pub static GMTIME: Cap = Cap::new();
pub static STRFTIME: Cap = Cap::new();
/// Formatter.usesTime, .formatTime, .formatMessage; PercentStyle.usesTime, .format,
/// ._format; LogRecord.getMessage; LogRecord's `__dict__` descriptor.
pub static F_USES_TIME: Cap = Cap::new();
pub static F_FORMAT_TIME: Cap = Cap::new();
pub static F_FORMAT_MESSAGE: Cap = Cap::new();
pub static S_USES_TIME: Cap = Cap::new();
pub static S_FORMAT: Cap = Cap::new();
pub static S_FORMAT_: Cap = Cap::new();
pub static R_GET_MESSAGE: Cap = Cap::new();
pub static R_DICT: Cap = Cap::new();

/// formatTime.
pub static TIME_CODES: [FnGuard; 1] = [const { FnGuard::new() }; 1];
/// format, formatTime, usesTime, formatMessage, PercentStyle.usesTime/.format/._format,
/// getMessage.
pub static FORMAT_CODES: [FnGuard; 8] = [const { FnGuard::new() }; 8];

static TIME_FORMATTERS: TypeCache = TypeCache::new(validate_time_formatter);
static FORMATTERS: TypeCache = TypeCache::new(validate_formatter);
static STYLES: TypeCache = TypeCache::new(validate_style);
static FMT_RECORDS: TypeCache = TypeCache::new(validate_fmt_record);

/// `converter`, `default_time_format`, `default_msec_format` are plain values.
unsafe fn validate_time_formatter(t: Tp) -> bool {
    // SAFETY: attached thread; live type.
    unsafe {
        let n = n();
        generic_attrs(t, false)
            && plain_attr(t, o(n.converter))
            && plain_attr(t, o(n.default_time_format))
            && plain_attr(t, o(n.default_msec_format))
    }
}

unsafe fn validate_formatter(t: Tp) -> bool {
    // SAFETY: attached thread; live type.
    unsafe {
        let n = n();
        validate_time_formatter(t)
            && resolves_to(t, o(n.uses_time), F_USES_TIME.get())
            && resolves_to(t, o(n.format_time), F_FORMAT_TIME.get())
            && resolves_to(t, o(n.format_message), F_FORMAT_MESSAGE.get())
            && plain_attr(t, o(n.style))
            && plain_attr(t, o(n.datefmt))
    }
}

unsafe fn validate_style(t: Tp) -> bool {
    // SAFETY: attached thread; live type.
    unsafe {
        let n = n();
        generic_attrs(t, false)
            && resolves_to(t, o(n.uses_time), S_USES_TIME.get())
            && resolves_to(t, o(n.format), S_FORMAT.get())
            && resolves_to(t, o(n.format_), S_FORMAT_.get())
            && plain_attr(t, o(n.fmt))
            && plain_attr(t, o(n.defaults))
            && plain_attr(t, o(n.asctime_search))
    }
}

unsafe fn validate_fmt_record(t: Tp) -> bool {
    // SAFETY: attached thread; live type.
    unsafe {
        RECORDS.ok(t)
            && resolves_to(t, o(n().get_message), R_GET_MESSAGE.get())
            && resolves_to(t, ffi::PyUnicode_InternFromString(c"__dict__".as_ptr()), R_DICT.get())
    }
}

// ---------------------------------------------------------------------------------------
// Broken-down time
// ---------------------------------------------------------------------------------------

/// `struct tm` prefix (identical on glibc, musl and BSD libc).
#[cfg(unix)]
#[repr(C)]
struct CTm {
    tm_sec: i32,
    tm_min: i32,
    tm_hour: i32,
    tm_mday: i32,
    tm_mon: i32,
    tm_year: i32,
    tm_wday: i32,
    tm_yday: i32,
    tm_isdst: i32,
    _rest: [u64; 4],
}

#[cfg(unix)]
unsafe extern "C" {
    fn localtime_r(t: *const i64, tm: *mut CTm) -> *mut CTm;
    fn gmtime_r(t: *const i64, tm: *mut CTm) -> *mut CTm;
}

/// `time.localtime(created)` / `time.gmtime(created)` for a float: floor to `time_t`
/// (`_PyTime_ObjectToTime_t`, `ROUND_FLOOR`) and the same libc call.
#[cfg(unix)]
unsafe fn broken_down(created: f64, gm: bool) -> Option<Tm> {
    if !created.is_finite() {
        return None;
    }
    let t = created.floor();
    if !(-1e11..=1e11).contains(&t) {
        return None;
    }
    let t = t as i64;
    // SAFETY: zeroed out-parameter; reentrant libc functions.
    unsafe {
        let mut tm: CTm = std::mem::zeroed();
        let r = if gm { gmtime_r(&t, &mut tm) } else { localtime_r(&t, &mut tm) };
        if r.is_null() {
            return None;
        }
        Some(Tm {
            year: tm.tm_year as i64 + 1900,
            mon: tm.tm_mon + 1,
            mday: tm.tm_mday,
            hour: tm.tm_hour,
            min: tm.tm_min,
            sec: tm.tm_sec,
            yday: tm.tm_yday + 1,
        })
    }
}

#[cfg(not(unix))]
unsafe fn broken_down(_created: f64, _gm: bool) -> Option<Tm> {
    None
}

/// The Latin-1 bytes of an exact str of kind 1 (no copy), or None.
unsafe fn latin1(s: O) -> Option<&'static [u8]> {
    // SAFETY: exact str; the slice is used while `s` is alive (callers hold it).
    unsafe {
        if ffi::PyUnicode_KIND(s) != 1 {
            return None;
        }
        Some(std::slice::from_raw_parts(ffi::PyUnicode_DATA(s).cast::<u8>(), ffi::PyUnicode_GET_LENGTH(s) as usize))
    }
}

/// `time.strftime(fmt, conv(created))` through the C functions themselves.
unsafe fn strftime_generic(conv: O, created_obj: O, fmt: O) -> Option<O> {
    // SAFETY: attached thread; C functions with plain arguments.
    unsafe {
        let ct = ffi::PyObject_CallOneArg(conv, created_obj);
        if ct.is_null() {
            ffi::PyErr_Clear();
            return None;
        }
        let a = [fmt, ct];
        let s = ffi::PyObject_Vectorcall(STRFTIME.get(), a.as_ptr(), 2, std::ptr::null_mut());
        ffi::Py_DECREF(ct);
        if s.is_null() {
            ffi::PyErr_Clear();
            return None;
        }
        Some(s)
    }
}

/// `Formatter.formatTime(self, record, datefmt)`: `Some(new str)` or `None` (fall back).
///
/// # Safety
/// Attached thread; `me`/`record` of validated types (time formatter, record).
pub unsafe fn format_time(me: O, record: O, datefmt: O) -> Option<O> {
    // SAFETY: per the caller; every call below is a plain read or a C function.
    unsafe {
        let n = n();
        let conv = get(me, o(n.converter));
        if conv.is_null() {
            return None;
        }
        ffi::Py_DECREF(conv); // identity only: LOCALTIME/GMTIME are kept alive
        let gm = if conv == LOCALTIME.get() {
            false
        } else if conv == GMTIME.get() {
            true
        } else {
            return None;
        };
        let created_obj = get(record, o(n.created));
        if created_obj.is_null() {
            return None;
        }
        let created_g = Owned1(created_obj);
        if !is_type(created_obj, &raw mut ffi::PyFloat_Type) {
            return None;
        }
        let tm = broken_down(ffi::PyFloat_AS_DOUBLE(created_obj), gm);
        // `if datefmt:`
        let with_datefmt = match pure_truth(datefmt) {
            Some(b) if datefmt == ffi::Py_None() || exact_str(datefmt) => b,
            _ => return None,
        };
        let mut out: Vec<u8> = Vec::with_capacity(32);
        if with_datefmt {
            if let (Some(tm), Some(b)) = (tm, latin1(datefmt))
                && strftime_numeric(b, &tm, &mut out)
            {
                return new_ascii(&out);
            }
            return strftime_generic(conv, created_g.0, datefmt);
        }
        let fmt = get(me, o(n.default_time_format));
        if fmt.is_null() {
            return None;
        }
        let fmt = Owned1(fmt);
        let msf = get(me, o(n.default_msec_format));
        if msf.is_null() {
            return None;
        }
        let msf = Owned1(msf);
        if !exact_str(fmt.0) {
            return None;
        }
        let with_msecs = match pure_truth(msf.0) {
            Some(false) => false,
            Some(true) if exact_str(msf.0) => true,
            _ => return None,
        };
        let msecs = if with_msecs {
            let m = get(record, o(n.msecs));
            if m.is_null() {
                return None;
            }
            Owned1(m)
        } else {
            Owned1(std::ptr::null_mut())
        };
        if with_msecs && !pure_scalar(msecs.0) {
            return None;
        }
        let native = match (tm, latin1(fmt.0)) {
            (Some(tm), Some(b)) => strftime_numeric(b, &tm, &mut out),
            _ => false,
        };
        if !with_msecs {
            return if native { new_ascii(&out) } else { strftime_generic(conv, created_g.0, fmt.0) };
        }
        // The default '%s,%03d' with a float msecs in range: one string.
        if native
            && is_type(msecs.0, &raw mut ffi::PyFloat_Type)
            && ffi::PyUnicode_Compare(msf.0, o(n.msec_default)) == 0
            && msec_suffix(ffi::PyFloat_AS_DOUBLE(msecs.0), &mut out)
        {
            return new_ascii(&out);
        }
        let s = if native { new_ascii(&out)? } else { strftime_generic(conv, created_g.0, fmt.0)? };
        let s = Owned1(s);
        let t = ffi::PyTuple_Pack(2, s.0, msecs.0);
        if t.is_null() {
            ffi::PyErr_Clear();
            return None;
        }
        let r = ffi::PyUnicode_Format(msf.0, t);
        ffi::Py_DECREF(t);
        if r.is_null() {
            ffi::PyErr_Clear();
            return None;
        }
        Some(r)
    }
}

/// A str from ASCII bytes.
unsafe fn new_ascii(b: &[u8]) -> Option<O> {
    // SAFETY: attached thread; ASCII (valid UTF-8) bytes.
    unsafe {
        let r = ffi::PyUnicode_FromStringAndSize(b.as_ptr().cast(), b.len() as _);
        if r.is_null() {
            ffi::PyErr_Clear();
            return None;
        }
        Some(r)
    }
}

/// One owned reference, released on drop.
struct Owned1(O);

impl Owned1 {
    fn take(self) -> O {
        let p = self.0;
        std::mem::forget(self);
        p
    }
}

impl Drop for Owned1 {
    fn drop(&mut self) {
        // SAFETY: an owned reference (or NULL).
        unsafe { ffi::Py_XDECREF(self.0) };
    }
}

/// `Formatter.formatTime(self, record, datefmt=None)`.
pub unsafe extern "C" fn vc_format_time(func: O, args: *const O, nargsf: usize, kw: O) -> O {
    // SAFETY: vectorcall arguments; attached thread.
    unsafe {
        let nargs = nargs(nargsf);
        let why = 'fb: {
            if !kw.is_null() || !(2..=3).contains(&nargs) {
                break 'fb Reason::Args;
            }
            if !TIME_DEPS.ok() {
                break 'fb Reason::Deps;
            }
            let (me, record) = (*args, *args.add(1));
            if !TIME_FORMATTERS.ok(ffi::Py_TYPE(me)) || !RECORDS.ok(ffi::Py_TYPE(record)) {
                break 'fb Reason::Args;
            }
            if !TOOLS.quiet() || !fns_ok(&TIME_CODES) {
                break 'fb Reason::Tools;
            }
            let datefmt = if nargs == 3 { *args.add(2) } else { ffi::Py_None() };
            if let Some(s) = format_time(me, record, datefmt) {
                TIME_STATS.hit();
                return s;
            }
            Reason::Args
        };
        TIME_STATS.fallback(why);
        original(func, args, nargsf, kw)
    }
}

// ---------------------------------------------------------------------------------------
// Formatter.format
// ---------------------------------------------------------------------------------------

/// `record.getMessage()`: `str(msg) % args` for plain operands. `Some(new str)`.
unsafe fn message(record: O) -> Option<O> {
    // SAFETY: attached thread; record of a validated type.
    unsafe {
        let n = n();
        let msg = get(record, o(n.msg));
        if msg.is_null() {
            return None;
        }
        let s = if exact_str(msg) {
            msg
        } else if pure_scalar(msg) {
            let s = ffi::PyObject_Str(msg);
            ffi::Py_DECREF(msg);
            if s.is_null() {
                ffi::PyErr_Clear();
                return None;
            }
            s
        } else {
            ffi::Py_DECREF(msg);
            return None;
        };
        let s = Owned1(s);
        let args = get(record, o(n.args));
        if args.is_null() {
            return None;
        }
        let args = Owned1(args);
        match pure_truth(args.0) {
            Some(false) => return Some(s.take()),
            Some(true) => {}
            None => return None,
        }
        let plain = if exact_tuple(args.0) {
            (0..ffi::PyTuple_GET_SIZE(args.0)).all(|i| pure_scalar(ffi::PyTuple_GET_ITEM(args.0, i)))
        } else if exact_dict(args.0) {
            ffi::PyDict_Size(args.0) <= 64 && dict_all(args.0, |k, v| exact_str(k) && pure_scalar(v))
        } else {
            false
        };
        if !plain {
            return None;
        }
        let r = ffi::PyUnicode_Format(s.0, args.0);
        if r.is_null() {
            ffi::PyErr_Clear();
            return None;
        }
        Some(r)
    }
}

/// The key `fmt[a..b]` (Latin-1 bytes) equals the exact str `k`.
unsafe fn key_is(fmt: &[u8], a: usize, b: usize, k: O) -> bool {
    // SAFETY: exact str.
    unsafe {
        if ffi::PyUnicode_GET_LENGTH(k) as usize != b - a {
            return false;
        }
        (a..b).enumerate().all(|(j, i)| ffi::PyUnicode_ReadChar(k, j as _) == fmt[i] as u32)
    }
}

enum Fmt {
    Done(O),
    /// Nothing observable happened, or only stores the original repeats.
    Fallback(Reason),
}

unsafe fn format(me: O, record: O) -> Fmt {
    // SAFETY: attached thread; `me`, `record` of validated types.
    unsafe {
        let n = n();
        macro_rules! fb {
            ($r:expr) => {
                return Fmt::Fallback($r)
            };
        }
        if !bound_to(me, o(n.uses_time), F_USES_TIME.get())
            || !bound_to(me, o(n.format_time), F_FORMAT_TIME.get())
            || !bound_to(me, o(n.format_message), F_FORMAT_MESSAGE.get())
            || !bound_to(record, o(n.get_message), R_GET_MESSAGE.get())
        {
            fb!(Reason::Args);
        }
        // exc_info / exc_text / stack_info must all be false: formatException and
        // formatStack (traceback formatting) stay CPython's.
        for a in [n.exc_info, n.exc_text, n.stack_info] {
            let v = get(record, o(a));
            if v.is_null() {
                fb!(Reason::Args);
            }
            let t = pure_truth(v);
            ffi::Py_DECREF(v);
            if t != Some(false) {
                fb!(Reason::Args);
            }
        }
        let style = get(me, o(n.style));
        if style.is_null() {
            fb!(Reason::Args);
        }
        let style = Owned1(style);
        if !STYLES.ok(ffi::Py_TYPE(style.0))
            || !bound_to(style.0, o(n.uses_time), S_USES_TIME.get())
            || !bound_to(style.0, o(n.format), S_FORMAT.get())
            || !bound_to(style.0, o(n.format_), S_FORMAT_.get())
        {
            fb!(Reason::Args);
        }
        let Some(msg) = message(record) else { fb!(Reason::Args) };
        let msg = Owned1(msg);
        // usesTime: self._style._fmt.find(self._style.asctime_search) >= 0
        let sfmt = get(style.0, o(n.fmt));
        if sfmt.is_null() {
            fb!(Reason::Args);
        }
        let sfmt = Owned1(sfmt);
        let search = get(style.0, o(n.asctime_search));
        if search.is_null() {
            fb!(Reason::Args);
        }
        let search = Owned1(search);
        if !exact_str(sfmt.0) || !exact_str(search.0) {
            fb!(Reason::Args);
        }
        let pos = ffi::PyUnicode_Find(sfmt.0, search.0, 0, ffi::PY_SSIZE_T_MAX, 1);
        if pos < -1 {
            ffi::PyErr_Clear();
            fb!(Reason::Args);
        }
        let uses_time = pos >= 0;
        let asctime = if uses_time {
            let datefmt = get(me, o(n.datefmt));
            if datefmt.is_null() {
                fb!(Reason::Args);
            }
            let datefmt = Owned1(datefmt);
            let Some(a) = format_time(me, record, datefmt.0) else { fb!(Reason::Args) };
            Owned1(a)
        } else {
            Owned1(std::ptr::null_mut())
        };
        // values = defaults | record.__dict__ if defaults else record.__dict__
        let defaults = get(style.0, o(n.defaults));
        if defaults.is_null() {
            fb!(Reason::Args);
        }
        let defaults = Owned1(defaults);
        let merge = match pure_truth(defaults.0) {
            Some(false) => false,
            Some(true) if exact_dict(defaults.0) => true,
            _ => fb!(Reason::Args),
        };
        let Some(fbytes) = latin1(sfmt.0) else { fb!(Reason::Args) };
        let Some(keys) = percent_keys(fbytes) else { fb!(Reason::Args) };
        // Observable from here on: stores into the record (the original repeats them).
        if ffi::PyObject_SetAttr(record, o(n.message), msg.0) < 0 {
            ffi::PyErr_Clear();
            fb!(Reason::Deopt);
        }
        if uses_time && ffi::PyObject_SetAttr(record, o(n.asctime), asctime.0) < 0 {
            ffi::PyErr_Clear();
            fb!(Reason::Deopt);
        }
        let d = ffi::PyObject_GenericGetDict(record, std::ptr::null_mut());
        if d.is_null() {
            ffi::PyErr_Clear();
            fb!(Reason::Deopt);
        }
        let mut values = Owned1(d);
        if merge {
            let v = ffi::PyDict_Copy(defaults.0);
            if v.is_null() || ffi::PyDict_Update(v, values.0) < 0 {
                ffi::PyErr_Clear();
                ffi::Py_XDECREF(v);
                fb!(Reason::Deopt);
            }
            values = Owned1(v);
        }
        // Every key a str; the keys the format reads hold plain values.
        let mut impure: [O; 8] = [std::ptr::null_mut(); 8];
        let mut ni = 0;
        let ok = ffi::PyDict_Size(values.0) <= 256
            && dict_all(values.0, |k, v| {
                if !exact_str(k) {
                    return false;
                }
                if !pure_scalar(v) {
                    if ni == impure.len() {
                        return false;
                    }
                    impure[ni] = k;
                    ni += 1;
                }
                true
            });
        if !ok {
            fb!(Reason::Deopt);
        }
        for &(a, b) in &keys {
            if impure[..ni].iter().any(|&k| key_is(fbytes, a, b, k)) {
                fb!(Reason::Deopt);
            }
        }
        let s = ffi::PyUnicode_Format(sfmt.0, values.0);
        if s.is_null() {
            ffi::PyErr_Clear();
            fb!(Reason::Deopt);
        }
        Fmt::Done(s)
    }
}

/// `Formatter.format(self, record)`.
pub unsafe extern "C" fn vc_format(func: O, args: *const O, nargsf: usize, kw: O) -> O {
    // SAFETY: vectorcall arguments; attached thread.
    unsafe {
        let why = 'fb: {
            if !kw.is_null() || nargs(nargsf) != 2 {
                break 'fb Reason::Args;
            }
            if !FORMAT_DEPS.ok() || !TIME_DEPS.ok() {
                break 'fb Reason::Deps;
            }
            let (me, record) = (*args, *args.add(1));
            if !FORMATTERS.ok(ffi::Py_TYPE(me)) || !FMT_RECORDS.ok(ffi::Py_TYPE(record)) {
                break 'fb Reason::Args;
            }
            if !TOOLS.quiet() || !fns_ok(&FORMAT_CODES) {
                break 'fb Reason::Tools;
            }
            match format(me, record) {
                Fmt::Done(s) => {
                    FORMAT_STATS.hit();
                    return s;
                }
                Fmt::Fallback(r) => r,
            }
        };
        FORMAT_STATS.fallback(why);
        original(func, args, nargsf, kw)
    }
}

pub fn clear_type_caches() {
    TIME_FORMATTERS.clear();
    FORMATTERS.clear();
    STYLES.clear();
    FMT_RECORDS.clear();
}
