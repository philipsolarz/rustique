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
use rustique_core::logging::{
    Dir, Piece, Tm, Val, dirs, msec_suffix, native_dir, parse_format, render, strftime_numeric,
};

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

/// Reads converter, default_time_format, default_msec_format.
pub static TIME_FORMATTERS: TypeCache<3> = TypeCache::new(validate_time_formatter);
/// Reads converter, default_time_format, default_msec_format, _style, datefmt.
pub static FORMATTERS: TypeCache<5> = TypeCache::new(validate_formatter);
/// Reads _fmt, asctime_search, _defaults.
pub static STYLES: TypeCache<3> = TypeCache::new(validate_style);
pub static FMT_RECORDS: TypeCache<0> = TypeCache::new(validate_fmt_record);

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
pub unsafe fn format_time(fe: usize, me: O, re: usize, record: O, datefmt: O) -> Option<O> {
    // SAFETY: per the caller.
    unsafe {
        format_time_with(
            &|k| TIME_FORMATTERS.read(fe, me, k),
            &|k| {
                let v = RECORDS.read(re, record, k);
                (!v.is_null()).then_some(v)
            },
            datefmt,
        )
    }
}

/// [`format_time`] reading the formatter's converter / default_time_format /
/// default_msec_format through `me` (0, 1, 2; new references or NULL) and the record's
/// created / msecs through `rec` (0, 1).
///
/// # Safety
/// As [`format_time`].
unsafe fn format_time_with(me: &dyn Fn(usize) -> O, rec: &dyn Fn(usize) -> Option<O>, datefmt: O) -> Option<O> {
    // SAFETY: per the caller; every call below is a plain read or a C function.
    unsafe {
        let n = n();
        let conv = me(0);
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
        let created_obj = rec(0)?;
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
        let fmt = me(1);
        if fmt.is_null() {
            return None;
        }
        let fmt = Owned1(fmt);
        let msf = me(2);
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
        let msecs = if with_msecs { Owned1(rec(1)?) } else { Owned1(std::ptr::null_mut()) };
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
///
/// # Safety
/// Installed as the function's vectorcall: CPython calls it with vectorcall arguments.
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
            let (Some(fe), Some(re)) = (TIME_FORMATTERS.entry(ffi::Py_TYPE(me)), RECORDS.entry(ffi::Py_TYPE(record)))
            else {
                break 'fb Reason::Args;
            };
            if !TOOLS.quiet() || !fns_ok(&TIME_CODES) {
                break 'fb Reason::Tools;
            }
            let datefmt = if nargs == 3 { *args.add(2) } else { ffi::Py_None() };
            if let Some(s) = format_time(fe, me, re, record, datefmt) {
                if TIME_STATS.hit_check_streak() {
                    let a = std::slice::from_raw_parts(args, nargs);
                    return crate::accel::check::verify(&super::FORMAT_TIME, s, a, || original(func, args, nargsf, kw));
                }
                return s;
            }
            Reason::Args
        };
        TIME_STATS.fallback_streak(why);
        original(func, args, nargsf, kw)
    }
}

// ---------------------------------------------------------------------------------------
// %-formatting
// ---------------------------------------------------------------------------------------

/// Latin-1 bytes of `str(v)` for `%s`, or the integer for `%d`, for plain values; `None`
/// when the native renderer does not handle the value (the generic path then decides).
/// Temporary strings are kept in `temps` (the bytes borrow them).
unsafe fn to_val(v: O, conv: u8, temps: &mut Vec<Owned1>) -> Option<Val<'static>> {
    // SAFETY: live object; returned bytes borrow `v` or a temp the caller keeps alive.
    unsafe {
        let t = ffi::Py_TYPE(v);
        if conv == b's' {
            if t == &raw mut ffi::PyUnicode_Type {
                return latin1(v).map(Val::Text);
            }
            if t == &raw mut ffi::PyLong_Type {
                let mut ov = 0;
                let x = ffi::PyLong_AsLongLongAndOverflow(v, &mut ov);
                if ov == 0 {
                    return Some(Val::Int(x));
                }
            }
            if v == ffi::Py_None() {
                return Some(Val::Text(b"None"));
            }
            if v == ffi::Py_True() {
                return Some(Val::Text(b"True"));
            }
            if v == ffi::Py_False() {
                return Some(Val::Text(b"False"));
            }
            if t == &raw mut ffi::PyFloat_Type || t == &raw mut ffi::PyLong_Type {
                let s = ffi::PyObject_Str(v);
                if s.is_null() {
                    ffi::PyErr_Clear();
                    return None;
                }
                temps.push(Owned1(s));
                return latin1(s).map(Val::Text);
            }
            return None;
        }
        if t == &raw mut ffi::PyLong_Type {
            let mut ov = 0;
            let x = ffi::PyLong_AsLongLongAndOverflow(v, &mut ov);
            return (ov == 0).then_some(Val::Int(x));
        }
        if v == ffi::Py_True() {
            return Some(Val::Int(1));
        }
        if v == ffi::Py_False() {
            return Some(Val::Int(0));
        }
        if t == &raw mut ffi::PyFloat_Type {
            let x = ffi::PyFloat_AS_DOUBLE(v);
            // int(x) truncates; NaN/inf raise (left to the generic path)
            if x.is_finite() && x.abs() < 9.0e18 {
                return Some(Val::Int(x.trunc() as i64));
            }
        }
        None
    }
}

/// `fmt % args` where every argument the format reads is plain. `vals` yields the value of
/// each directive in order (borrowed; `None` if missing). `Some(new str)`, or `None` when
/// the operation would raise or read a non-plain value (nothing observable happened).
unsafe fn percent(
    fmt: O,
    args: O,
    pieces: Option<&[Piece]>,
    value: &mut dyn FnMut(usize, &Dir) -> Option<O>,
) -> Option<O> {
    // SAFETY: attached thread; plain objects.
    unsafe {
        let mut temps: Vec<Owned1> = Vec::new();
        let mut held: Vec<Owned1> = Vec::new();
        let mut plain = true;
        let mut native = pieces.is_some();
        let mut vals: Vec<Val<'static>> = Vec::new();
        if let Some(pieces) = pieces {
            for (i, d) in dirs(pieces).enumerate() {
                let v = value(i, d)?;
                held.push(Owned1(v));
                if !pure_scalar(v) {
                    plain = false;
                    break;
                }
                if native {
                    match to_val(v, d.conv, &mut temps) {
                        Some(x) if native_dir(d) => vals.push(x),
                        _ => native = false,
                    }
                }
            }
        }
        if !plain {
            return None;
        }
        if native
            && let Some(pieces) = pieces
            && let Some(fb) = latin1(fmt)
        {
            let mut out: Vec<u8> = Vec::with_capacity(fb.len() + 64);
            if render(fb, pieces, &vals, &mut out) {
                let r = ffi::PyUnicode_DecodeLatin1(out.as_ptr().cast(), out.len() as _, std::ptr::null());
                if r.is_null() {
                    ffi::PyErr_Clear();
                    return None;
                }
                return Some(r);
            }
        }
        drop(held);
        let r = ffi::PyUnicode_Format(fmt, args);
        if r.is_null() {
            ffi::PyErr_Clear();
            return None;
        }
        Some(r)
    }
}

// ---------------------------------------------------------------------------------------
// Formatter.format
// ---------------------------------------------------------------------------------------

/// A parsed `PercentStyle._fmt`: its pieces and one key object per directive.
struct Parsed {
    fmt: usize,
    pieces: Vec<Piece>,
    keys: Vec<usize>,
}

impl Drop for Parsed {
    fn drop(&mut self) {
        // SAFETY: strong references taken in `parse_fmt`; dropped by an attached thread.
        unsafe {
            ffi::Py_DECREF(self.fmt as O);
            for &k in &self.keys {
                ffi::Py_DECREF(k as O);
            }
        }
    }
}

// SAFETY: plain addresses of objects whose references the entry owns.
unsafe impl Send for Parsed {}
// SAFETY: immutable after construction.
unsafe impl Sync for Parsed {}

static FMT_CACHE: parking_lot::Mutex<Vec<std::sync::Arc<Parsed>>> = parking_lot::Mutex::new(Vec::new());

/// Parse `fmt` (an exact str of kind 1) for a mapping: every directive keyed. Cached by
/// identity (the entry holds `fmt`, so the address cannot be reused while cached).
unsafe fn parse_fmt(fmt: O) -> Option<std::sync::Arc<Parsed>> {
    // SAFETY: attached thread.
    unsafe {
        if let Some(c) = FMT_CACHE.try_lock()
            && let Some(p) = c.iter().find(|p| p.fmt == fmt as usize)
        {
            return Some(p.clone());
        }
        let b = latin1(fmt)?;
        let pieces = parse_format(b)?;
        let mut keys = Vec::new();
        for d in dirs(&pieces) {
            let (x, y) = d.key?;
            let k = ffi::PyUnicode_DecodeLatin1(b[x..].as_ptr().cast(), (y - x) as _, std::ptr::null());
            if k.is_null() {
                ffi::PyErr_Clear();
                for &k in &keys {
                    ffi::Py_DECREF(k as O);
                }
                return None;
            }
            keys.push(k as usize);
        }
        ffi::Py_INCREF(fmt);
        let p = std::sync::Arc::new(Parsed { fmt: fmt as usize, pieces, keys });
        if let Some(mut c) = FMT_CACHE.try_lock() {
            if c.len() >= 16 {
                c.remove(0);
            }
            c.push(p.clone());
        }
        Some(p)
    }
}

/// `record.getMessage()`: `str(msg) % args` for plain operands. `Some(new str)`.
unsafe fn message(msg: O, args: O) -> Option<O> {
    // SAFETY: attached thread; borrowed operands.
    unsafe {
        let s = if exact_str(msg) {
            ffi::Py_NewRef(msg)
        } else if pure_scalar(msg) {
            let s = ffi::PyObject_Str(msg);
            if s.is_null() {
                ffi::PyErr_Clear();
                return None;
            }
            s
        } else {
            return None;
        };
        let s = Owned1(s);
        match pure_truth(args) {
            Some(false) => return Some(s.take()),
            Some(true) => {}
            None => return None,
        }
        let pieces = latin1(s.0).and_then(parse_format);
        if exact_tuple(args) {
            let n = ffi::PyTuple_GET_SIZE(args) as usize;
            if let Some(p) = &pieces
                && (dirs(p).count() != n || dirs(p).any(|d| d.key.is_some()))
            {
                return None; // TypeError in CPython
            }
            if pieces.is_none() && !(0..n).all(|i| pure_scalar(ffi::PyTuple_GET_ITEM(args, i as _))) {
                return None;
            }
            percent(s.0, args, pieces.as_deref(), &mut |i, _| Some(ffi::Py_NewRef(ffi::PyTuple_GET_ITEM(args, i as _))))
        } else if exact_dict(args) {
            if !str_keyed(args, 256) {
                return None;
            }
            if let Some(p) = &pieces
                && dirs(p).any(|d| d.key.is_none())
            {
                return None; // `%s` would format the whole dict
            }
            if pieces.is_none() && !dict_all(args, |_, v| pure_scalar(v)) {
                return None;
            }
            let fb = latin1(s.0);
            percent(s.0, args, pieces.as_deref(), &mut |_, d| {
                let (x, y) = d.key?;
                let fb = fb?;
                let k = ffi::PyUnicode_DecodeLatin1(fb[x..].as_ptr().cast(), (y - x) as _, std::ptr::null());
                if k.is_null() {
                    ffi::PyErr_Clear();
                    return None;
                }
                let mut v: O = std::ptr::null_mut();
                let r = ffi::PyDict_GetItemRef(args, k, &mut v);
                ffi::Py_DECREF(k);
                if r != 1 {
                    ffi::PyErr_Clear();
                    return None;
                }
                Some(v)
            })
        } else {
            None
        }
    }
}

enum Fmt {
    Done(O),
    /// Nothing observable happened, or only stores the original repeats.
    Fallback(Reason),
}

/// `d[key]` (new reference) for an exact dict with str keys.
#[inline(always)]
unsafe fn dget(d: O, key: O) -> Option<O> {
    // SAFETY: attached thread; str keys only (checked by the caller).
    unsafe {
        let mut v: O = std::ptr::null_mut();
        if ffi::PyDict_GetItemRef(d, key, &mut v) != 1 {
            ffi::PyErr_Clear();
            return None;
        }
        Some(v)
    }
}

unsafe fn format(fe: usize, me: O, record: O) -> Fmt {
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
        // `record.__dict__` (CPython materialises it in PercentStyle._format; doing it first
        // is unobservable). Record attributes are read from it: the record class has no
        // descriptor for them (validated), so the instance dict is what getattr reads.
        let d = ffi::PyObject_GenericGetDict(record, std::ptr::null_mut());
        if d.is_null() {
            ffi::PyErr_Clear();
            fb!(Reason::Args);
        }
        let d = Owned1(d);
        if !exact_dict(d.0) || !str_keyed(d.0, 256) {
            fb!(Reason::Args);
        }
        // exc_info / exc_text / stack_info must all be false: formatException and
        // formatStack (traceback formatting) stay CPython's.
        for a in [n.exc_info, n.exc_text, n.stack_info] {
            let Some(v) = dget(d.0, o(a)) else { fb!(Reason::Args) };
            let t = pure_truth(v);
            ffi::Py_DECREF(v);
            if t != Some(false) {
                fb!(Reason::Args);
            }
        }
        let style = FORMATTERS.read(fe, me, 3);
        if style.is_null() {
            fb!(Reason::Args);
        }
        let style = Owned1(style);
        let Some(se) = STYLES.entry(ffi::Py_TYPE(style.0)) else { fb!(Reason::Args) };
        if !bound_to(style.0, o(n.uses_time), S_USES_TIME.get())
            || !bound_to(style.0, o(n.format), S_FORMAT.get())
            || !bound_to(style.0, o(n.format_), S_FORMAT_.get())
        {
            fb!(Reason::Args);
        }
        let Some(msg) = dget(d.0, o(n.msg)) else { fb!(Reason::Args) };
        let msg_o = Owned1(msg);
        let Some(args) = dget(d.0, o(n.args)) else { fb!(Reason::Args) };
        let args_o = Owned1(args);
        let Some(message) = message(msg_o.0, args_o.0) else { fb!(Reason::Args) };
        let message = Owned1(message);
        // usesTime: self._style._fmt.find(self._style.asctime_search) >= 0
        let sfmt = STYLES.read(se, style.0, 0);
        if sfmt.is_null() {
            fb!(Reason::Args);
        }
        let sfmt = Owned1(sfmt);
        let search = STYLES.read(se, style.0, 1);
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
            let datefmt = FORMATTERS.read(fe, me, 4);
            if datefmt.is_null() {
                fb!(Reason::Args);
            }
            let datefmt = Owned1(datefmt);
            let rnames = [n.created, n.msecs];
            let Some(a) = format_time_with(&|k| FORMATTERS.read(fe, me, k), &|k| dget(d.0, o(rnames[k])), datefmt.0)
            else {
                fb!(Reason::Args)
            };
            Owned1(a)
        } else {
            Owned1(std::ptr::null_mut())
        };
        // values = defaults | record.__dict__ if defaults else record.__dict__
        let defaults = STYLES.read(se, style.0, 2);
        if defaults.is_null() {
            fb!(Reason::Args);
        }
        let defaults = Owned1(defaults);
        let merge = match pure_truth(defaults.0) {
            Some(false) => false,
            Some(true) if exact_dict(defaults.0) => true,
            _ => fb!(Reason::Args),
        };
        let Some(parsed) = parse_fmt(sfmt.0) else { fb!(Reason::Args) };
        // Observable from here on: stores into the record (the original repeats them).
        if ffi::PyObject_GenericSetAttr(record, o(n.message), message.0) < 0 {
            ffi::PyErr_Clear();
            fb!(Reason::Deopt);
        }
        if uses_time && ffi::PyObject_GenericSetAttr(record, o(n.asctime), asctime.0) < 0 {
            ffi::PyErr_Clear();
            fb!(Reason::Deopt);
        }
        let values = if merge {
            let v = ffi::PyDict_Copy(defaults.0);
            if v.is_null() || ffi::PyDict_Update(v, d.0) < 0 {
                ffi::PyErr_Clear();
                ffi::Py_XDECREF(v);
                fb!(Reason::Deopt);
            }
            Owned1(v)
        } else {
            Owned1(ffi::Py_NewRef(d.0))
        };
        if !str_keyed(values.0, 256) {
            fb!(Reason::Deopt);
        }
        let keys = &parsed.keys;
        match percent(sfmt.0, values.0, Some(&parsed.pieces), &mut |i, _| dget(values.0, keys[i] as O)) {
            Some(s) => Fmt::Done(s),
            None => Fmt::Fallback(Reason::Deopt),
        }
    }
}

pub fn clear_fmt_cache() {
    FMT_CACHE.lock().clear();
}

/// `Formatter.format(self, record)`.
///
/// # Safety
/// Installed as the function's vectorcall: CPython calls it with vectorcall arguments.
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
            let Some(fe) = FORMATTERS.entry(ffi::Py_TYPE(me)) else { break 'fb Reason::Args };
            if !FMT_RECORDS.ok(ffi::Py_TYPE(record)) {
                break 'fb Reason::Args;
            }
            if !TOOLS.quiet() || !fns_ok(&FORMAT_CODES) {
                break 'fb Reason::Tools;
            }
            match format(fe, me, record) {
                Fmt::Done(s) => {
                    FORMAT_STATS.hit_streak();
                    return s;
                }
                Fmt::Fallback(r) => r,
            }
        };
        FORMAT_STATS.fallback_streak(why);
        original(func, args, nargsf, kw)
    }
}

pub fn clear_type_caches() {
    TIME_FORMATTERS.clear();
    FORMATTERS.clear();
    STYLES.clear();
    FMT_RECORDS.clear();
}
