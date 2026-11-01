//! `datetime.isoformat` (also `str(datetime)`), `date.isoformat` (also `str(date)`) and
//! `time.isoformat` (also `str(time)`): C methods that format with `PyUnicode_FromFormat`
//! and append `format_utcoffset(tzinfo.utcoffset(...))`. The kernels write the same text in
//! one pass for exact `datetime`/`date`/`time` receivers, ASCII `sep`, a known `timespec`,
//! and tzinfo `None`, an exact `timezone` or an exact `zoneinfo.ZoneInfo` (datetimes only).

use pyo3::ffi;

use super::format::{self, FastKw, VarKw};
use super::{capi, ustr};
use crate::accel::{Counters, Original, Reason, check};

type O = *mut ffi::PyObject;
pub type NoArgs = unsafe extern "C" fn(O, O) -> O;

pub static C_DATETIME: Counters = Counters::new();
pub static C_DATE: Counters = Counters::new();
pub static C_TIME: Counters = Counters::new();
pub static ORIG_DATETIME_FAST: Original<FastKw> = Original::new();
pub static ORIG_DATETIME_VAR: Original<VarKw> = Original::new();
pub static ORIG_DATE: Original<NoArgs> = Original::new();
pub static ORIG_TIME_FAST: Original<FastKw> = Original::new();
pub static ORIG_TIME_VAR: Original<VarKw> = Original::new();

/// Interned "hours", "minutes", "seconds", "milliseconds", "microseconds", "auto" (filled at
/// install). `timespec` values are matched by identity only: call sites pass literals
/// (interned); any other object, including unknown values (a `ValueError`), falls back
/// after a few pointer compares.
pub static TIMESPECS: [std::sync::atomic::AtomicUsize; 6] = [const { std::sync::atomic::AtomicUsize::new(0) }; 6];
pub const TIMESPEC_NAMES: [&str; 6] = ["hours", "minutes", "seconds", "milliseconds", "microseconds", "auto"];

/// `timespec`: 0 hours, 1 minutes, 2 seconds, 3 milliseconds, 4 microseconds.
#[inline(always)]
fn spec(ts: O, us: u32) -> Option<u8> {
    let auto = if us == 0 { 2 } else { 4 };
    if ts.is_null() {
        return Some(auto);
    }
    let i = TIMESPECS.iter().position(|t| t.load(std::sync::atomic::Ordering::Relaxed) == ts as usize)?;
    Some(if i == 5 { auto } else { i as u8 })
}

/// Output bytes (uninitialized storage: only the written prefix is read).
struct Buf {
    b: std::mem::MaybeUninit<[u8; 96]>,
    n: usize,
}

impl Buf {
    #[inline(always)]
    fn new() -> Self {
        Buf { b: std::mem::MaybeUninit::uninit(), n: 0 }
    }
    #[inline(always)]
    fn push(&mut self, c: u8) {
        debug_assert!(self.n < 96);
        // SAFETY: n < 96 by construction (fixed formats of at most 32 bytes + offsets).
        unsafe { self.b.as_mut_ptr().cast::<u8>().add(self.n).write(c) };
        self.n += 1;
    }
    #[inline(always)]
    fn d2(&mut self, v: u32) {
        self.push(b'0' + (v / 10 % 10) as u8);
        self.push(b'0' + (v % 10) as u8);
    }
    #[inline(always)]
    fn d3(&mut self, v: u32) {
        self.push(b'0' + (v / 100 % 10) as u8);
        self.d2(v % 100);
    }
    #[inline(always)]
    fn d4(&mut self, v: u32) {
        self.d2(v / 100 % 100);
        self.d2(v % 100);
    }
    #[inline(always)]
    fn d6(&mut self, v: u32) {
        self.d3(v / 1000 % 1000);
        self.d3(v % 1000);
    }
    fn extend(&mut self, s: &[u8]) -> bool {
        if self.n + s.len() > 96 {
            return false;
        }
        // SAFETY: in bounds (checked).
        unsafe { std::ptr::copy_nonoverlapping(s.as_ptr(), self.b.as_mut_ptr().cast::<u8>().add(self.n), s.len()) };
        self.n += s.len();
        true
    }
    unsafe fn finish(&self) -> O {
        // SAFETY: the first n bytes are written ASCII.
        unsafe { ffi::PyUnicode_FromStringAndSize(self.b.as_ptr().cast(), self.n as ffi::Py_ssize_t) }
    }
}

/// "HH[:MM[:SS[.fff|.ffffff]]]" per `spec`.
fn time_part(b: &mut Buf, h: u32, m: u32, s: u32, us: u32, spec: u8) {
    b.d2(h);
    if spec >= 1 {
        b.push(b':');
        b.d2(m);
    }
    if spec >= 2 {
        b.push(b':');
        b.d2(s);
    }
    if spec == 3 {
        b.push(b'.');
        b.d3(us / 1000);
    } else if spec == 4 {
        b.push(b'.');
        b.d6(us);
    }
}

/// An exact ASCII `str` (or NULL: absent).
#[inline(always)]
unsafe fn ascii_arg<'a>(o: O) -> Option<Option<&'a [u8]>> {
    // SAFETY: live object or NULL.
    unsafe {
        if o.is_null() {
            return Some(None);
        }
        if ffi::PyUnicode_CheckExact(o) == 0 {
            return None;
        }
        ustr::ascii(o).map(Some)
    }
}

/// `datetime.isoformat(sep, timespec)` with optional (NULL) arguments.
#[inline(always)]
unsafe fn datetime_iso(this: O, sep: O, ts: O) -> Option<O> {
    // SAFETY: attached thread; live objects; C API filled.
    unsafe {
        if ffi::Py_TYPE(this) != (*capi()).DateTimeType {
            return None;
        }
        let sep = match ascii_arg(sep)? {
            None => b'T',
            Some([c]) => *c,
            Some(_) => return None,
        };
        let us = ffi::PyDateTime_DATE_GET_MICROSECOND(this) as u32;
        let spec = spec(ts, us)?;
        let mut b = Buf::new();
        b.d4(ffi::PyDateTime_GET_YEAR(this) as u32);
        b.push(b'-');
        b.d2(ffi::PyDateTime_GET_MONTH(this) as u32);
        b.push(b'-');
        b.d2(ffi::PyDateTime_GET_DAY(this) as u32);
        b.push(sep);
        time_part(
            &mut b,
            ffi::PyDateTime_DATE_GET_HOUR(this) as u32,
            ffi::PyDateTime_DATE_GET_MINUTE(this) as u32,
            ffi::PyDateTime_DATE_GET_SECOND(this) as u32,
            us,
            spec,
        );
        let tz = ffi::PyDateTime_DATE_GET_TZINFO(this);
        if tz != ffi::Py_None() {
            let off = format::utcoffset_colon(tz, this, true)?;
            if !b.extend(&off) {
                return None;
            }
        }
        Some(b.finish())
    }
}

/// `time.isoformat(timespec)`.
#[inline(always)]
unsafe fn time_iso(this: O, ts: O) -> Option<O> {
    // SAFETY: attached thread; live objects; C API filled.
    unsafe {
        if ffi::Py_TYPE(this) != (*capi()).TimeType {
            return None;
        }
        let us = ffi::PyDateTime_TIME_GET_MICROSECOND(this) as u32;
        let spec = spec(ts, us)?;
        let mut b = Buf::new();
        time_part(
            &mut b,
            ffi::PyDateTime_TIME_GET_HOUR(this) as u32,
            ffi::PyDateTime_TIME_GET_MINUTE(this) as u32,
            ffi::PyDateTime_TIME_GET_SECOND(this) as u32,
            us,
            spec,
        );
        let tz = ffi::PyDateTime_TIME_GET_TZINFO(this);
        if tz != ffi::Py_None() {
            let off = format::utcoffset_colon(tz, ffi::Py_None(), false)?;
            if !b.extend(&off) {
                return None;
            }
        }
        Some(b.finish())
    }
}

/// METH_NOARGS `date.isoformat`.
///
/// # Safety
/// Called by CPython with this calling convention's arguments.
pub unsafe extern "C" fn date_noargs(this: O, unused: O) -> O {
    // SAFETY: CPython's calling convention; fallback with unchanged arguments.
    unsafe {
        if ffi::Py_TYPE(this) == (*capi()).DateType {
            let mut b = Buf::new();
            b.d4(ffi::PyDateTime_GET_YEAR(this) as u32);
            b.push(b'-');
            b.d2(ffi::PyDateTime_GET_MONTH(this) as u32);
            b.push(b'-');
            b.d2(ffi::PyDateTime_GET_DAY(this) as u32);
            let r = b.finish();
            if C_DATE.hit_check() {
                return check::verify(&super::DATE_ISOFORMAT, r, &[this], || ORIG_DATE.get()(this, unused));
            }
            return r;
        }
        C_DATE.fallback(Reason::Args);
        ORIG_DATE.get()(this, unused)
    }
}

/// Interned "sep" / "timespec" (call sites pass interned keyword names: pointer compare
/// first).
pub static K_SEP: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
pub static K_TIMESPEC: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Positional/keyword arguments `(sep, timespec)` or `(timespec,)` of the FASTCALL
/// convention; `None` for anything else (duplicates, unknown names).
#[inline(always)]
unsafe fn fast_args<const N: usize>(
    names: [(&std::ffi::CStr, &std::sync::atomic::AtomicUsize); N],
    args: *const O,
    nargs: ffi::Py_ssize_t,
    kwnames: O,
) -> Option<[O; N]> {
    // SAFETY: CPython's FASTCALL arguments.
    unsafe {
        let nargs = nargs as usize;
        if nargs > N {
            return None;
        }
        let mut out = [std::ptr::null_mut(); N];
        for (i, o) in out.iter_mut().enumerate().take(nargs) {
            *o = *args.add(i);
        }
        if !kwnames.is_null() {
            let nkw = ffi::PyTuple_GET_SIZE(kwnames) as usize;
            for k in 0..nkw {
                let key = ffi::PyTuple_GET_ITEM(kwnames, k as ffi::Py_ssize_t);
                // Interned names by identity first, then by value.
                let slot = names
                    .iter()
                    .position(|&(_, k)| key as usize == k.load(std::sync::atomic::Ordering::Relaxed))
                    .or_else(|| names.iter().position(|&(n, _)| ffi::PyUnicode_EqualToUTF8(key, n.as_ptr()) == 1))?;
                if !out[slot].is_null() {
                    return None;
                }
                out[slot] = *args.add(nargs + k);
            }
        }
        Some(out)
    }
}

/// Same for METH_VARARGS | METH_KEYWORDS.
unsafe fn var_args<const N: usize>(names: [&std::ffi::CStr; N], args: O, kw: O) -> Option<[O; N]> {
    // SAFETY: CPython's VARARGS arguments.
    unsafe {
        let nargs = ffi::PyTuple_GET_SIZE(args) as usize;
        if nargs > N {
            return None;
        }
        let mut out = [std::ptr::null_mut(); N];
        for (i, o) in out.iter_mut().enumerate().take(nargs) {
            *o = ffi::PyTuple_GET_ITEM(args, i as ffi::Py_ssize_t);
        }
        if !kw.is_null() && ffi::PyDict_Size(kw) != 0 {
            let mut found = 0;
            for (i, n) in names.iter().enumerate() {
                let v = ffi::PyDict_GetItemString(kw, n.as_ptr());
                if !v.is_null() {
                    if !out[i].is_null() {
                        return None;
                    }
                    out[i] = v;
                    found += 1;
                }
            }
            if found != ffi::PyDict_Size(kw) {
                return None;
            }
        }
        Some(out)
    }
}

/// METH_FASTCALL | METH_KEYWORDS `datetime.isoformat` (3.15).
///
/// # Safety
/// Called by CPython with this calling convention's arguments.
pub unsafe extern "C" fn datetime_fast(this: O, args: *const O, nargs: ffi::Py_ssize_t, kwnames: O) -> O {
    // SAFETY: CPython's calling convention; fallback with unchanged arguments.
    unsafe {
        if let Some([sep, ts]) = fast_args([(c"sep", &K_SEP), (c"timespec", &K_TIMESPEC)], args, nargs, kwnames)
            && let Some(r) = datetime_iso(this, sep, ts)
        {
            if C_DATETIME.hit_check() {
                return check::verify(&super::DATETIME_ISOFORMAT, r, &[this], || {
                    ORIG_DATETIME_FAST.get()(this, args, nargs, kwnames)
                });
            }
            return r;
        }
        C_DATETIME.fallback(Reason::Args);
        ORIG_DATETIME_FAST.get()(this, args, nargs, kwnames)
    }
}

/// METH_VARARGS | METH_KEYWORDS `datetime.isoformat` (3.14).
///
/// # Safety
/// Called by CPython with this calling convention's arguments.
pub unsafe extern "C" fn datetime_var(this: O, args: O, kw: O) -> O {
    // SAFETY: CPython's calling convention; fallback with unchanged arguments.
    unsafe {
        if let Some([sep, ts]) = var_args([c"sep", c"timespec"], args, kw)
            && let Some(r) = datetime_iso(this, sep, ts)
        {
            if C_DATETIME.hit_check() {
                return check::verify(&super::DATETIME_ISOFORMAT, r, &[this], || {
                    ORIG_DATETIME_VAR.get()(this, args, kw)
                });
            }
            return r;
        }
        C_DATETIME.fallback(Reason::Args);
        ORIG_DATETIME_VAR.get()(this, args, kw)
    }
}

/// METH_FASTCALL | METH_KEYWORDS `time.isoformat` (3.15).
///
/// # Safety
/// Called by CPython with this calling convention's arguments.
pub unsafe extern "C" fn time_fast(this: O, args: *const O, nargs: ffi::Py_ssize_t, kwnames: O) -> O {
    // SAFETY: CPython's calling convention; fallback with unchanged arguments.
    unsafe {
        if let Some([ts]) = fast_args([(c"timespec", &K_TIMESPEC)], args, nargs, kwnames)
            && let Some(r) = time_iso(this, ts)
        {
            if C_TIME.hit_check() {
                return check::verify(&super::TIME_ISOFORMAT, r, &[this], || {
                    ORIG_TIME_FAST.get()(this, args, nargs, kwnames)
                });
            }
            return r;
        }
        C_TIME.fallback(Reason::Args);
        ORIG_TIME_FAST.get()(this, args, nargs, kwnames)
    }
}

/// METH_VARARGS | METH_KEYWORDS `time.isoformat` (3.14).
///
/// # Safety
/// Called by CPython with this calling convention's arguments.
pub unsafe extern "C" fn time_var(this: O, args: O, kw: O) -> O {
    // SAFETY: CPython's calling convention; fallback with unchanged arguments.
    unsafe {
        if let Some([ts]) = var_args([c"timespec"], args, kw)
            && let Some(r) = time_iso(this, ts)
        {
            if C_TIME.hit_check() {
                return check::verify(&super::TIME_ISOFORMAT, r, &[this], || ORIG_TIME_VAR.get()(this, args, kw));
            }
            return r;
        }
        C_TIME.fallback(Reason::Args);
        ORIG_TIME_VAR.get()(this, args, kw)
    }
}
