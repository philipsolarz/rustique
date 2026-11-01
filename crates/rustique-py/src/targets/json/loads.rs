//! Target `json.loads`: the vectorcall of the pure-Python `json.loads`.
//!
//! Exact subset: one positional argument, an exact `str` (not starting with a BOM) or an
//! exact `bytes` that `json.detect_encoding` reads as UTF-8 without a BOM (decoded with
//! `surrogatepass`, as `loads` does); keyword arguments only among the hooks and `cls`,
//! all `None` (the default decoder path). The document must then be in the decoder's subset
//! ([`super::decode`]); anything else, including every malformed document, re-runs the
//! original, so `JSONDecodeError` (message, `pos`, `lineno`, `colno`) is CPython's.

use std::sync::atomic::{AtomicPtr, AtomicUsize, Ordering::Relaxed};

use pyo3::ffi;

use super::decode::Parser;
use super::encode::Stop;
use super::pystr::{View, view};
use crate::accel::{self, Reason, check};

pub(super) static COUNTERS: accel::Counters = accel::Counters::new();
pub(super) static DEPS: accel::DepFlag = accel::DepFlag::new();
pub(super) static TOOLS: accel::Tools = accel::Tools::new();

/// Identity-checked per call (their contents are pinned): `json.loads`, its code and
/// keyword defaults, `json._default_decoder` and the `JSONDecoder` class.
pub(super) static FUNC: AtomicPtr<ffi::PyObject> = AtomicPtr::new(std::ptr::null_mut());
pub(super) static CODE: AtomicPtr<ffi::PyObject> = AtomicPtr::new(std::ptr::null_mut());
pub(super) static KWDEFAULTS: AtomicPtr<ffi::PyObject> = AtomicPtr::new(std::ptr::null_mut());
pub(super) static DEFAULT_DECODER: AtomicPtr<ffi::PyObject> = AtomicPtr::new(std::ptr::null_mut());
pub(super) static DECODER_TYPE: AtomicPtr<ffi::PyObject> = AtomicPtr::new(std::ptr::null_mut());

/// `JSONDecoder.decode` / `raw_decode`: code and defaults unchanged.
pub(super) static DECODE: super::FnCheck = super::FnCheck::new();
pub(super) static RAW_DECODE: super::FnCheck = super::FnCheck::new();

/// Interned keyword names accepted (with value `None`): the first `KW_COUNT` of `KW_NAMES`
/// (`array_hook` exists from 3.15).
pub(super) static KW: [AtomicPtr<ffi::PyObject>; 7] = [const { AtomicPtr::new(std::ptr::null_mut()) }; 7];
pub(super) static KW_COUNT: AtomicUsize = AtomicUsize::new(0);
pub(super) const KW_NAMES: [&str; 7] =
    ["cls", "object_hook", "parse_float", "parse_int", "parse_constant", "object_pairs_hook", "array_hook"];

/// Keyword arguments all `None` and all known.
#[inline(always)]
unsafe fn kwargs_ok(args: *const *mut ffi::PyObject, nargs: usize, kwnames: *mut ffi::PyObject) -> bool {
    if kwnames.is_null() {
        return true;
    }
    let known = &KW[..KW_COUNT.load(Relaxed)];
    // SAFETY: vectorcall convention.
    unsafe {
        let n = ffi::PyTuple_GET_SIZE(kwnames) as usize;
        for j in 0..n {
            if *args.add(nargs + j) != ffi::Py_None() {
                return false;
            }
            let name = ffi::PyTuple_GET_ITEM(kwnames, j as ffi::Py_ssize_t);
            if !known.iter().any(|k| k.load(Relaxed) == name)
                && !(ffi::Py_TYPE(name) == &raw mut ffi::PyUnicode_Type
                    && known.iter().any(|k| ffi::PyUnicode_Compare(name, k.load(Relaxed)) == 0))
            {
                return false;
            }
        }
    }
    true
}

/// Parse a string view.
///
/// # Safety
/// Attached thread; `v` views a live string.
unsafe fn parse_view(v: &View) -> Result<*mut ffi::PyObject, Stop> {
    // SAFETY: per the caller.
    unsafe {
        match v.kind {
            1 => Parser::new(v.u8s(), *v).document(),
            2 => Parser::new(v.u16s(), *v).document(),
            _ => Parser::new(v.u32s(), *v).document(),
        }
    }
}

/// `json.loads(s)` for the exact subset of inputs.
///
/// # Safety
/// Attached thread; live object.
pub(super) unsafe fn loads(s: *mut ffi::PyObject) -> Result<*mut ffi::PyObject, Stop> {
    // SAFETY: per the caller; type checks before layout reads.
    unsafe {
        let t = ffi::Py_TYPE(s);
        if t == &raw mut ffi::PyUnicode_Type {
            let v = view(s);
            // A leading U+FEFF: CPython raises "Unexpected UTF-8 BOM".
            if v.len > 0
                && ((v.kind == 2 && *v.data.cast::<u16>() == 0xFEFF)
                    || (v.kind == 4 && *v.data.cast::<u32>() == 0xFEFF))
            {
                return Err(Stop);
            }
            return parse_view(&v);
        }
        if t != &raw mut ffi::PyBytes_Type {
            return Err(Stop);
        }
        let len = ffi::Py_SIZE(s) as usize;
        let data = ffi::PyBytes_AS_STRING(s).cast::<u8>();
        let b = std::slice::from_raw_parts(data, len);
        // `json.detect_encoding`: anything but plain UTF-8 (BOMs, NUL patterns of UTF-16/32).
        if b.starts_with(&[0xEF, 0xBB, 0xBF])
            || b.starts_with(&[0xFE, 0xFF])
            || b.starts_with(&[0xFF, 0xFE])
            || b.starts_with(&[0x00, 0x00, 0xFE, 0xFF])
            || ((len >= 4 || len == 2) && (b[0] == 0 || b[1] == 0))
        {
            return Err(Stop);
        }
        if b.is_ascii() {
            // `s.decode('utf-8', 'surrogatepass')` is an ASCII string with these bytes.
            let v = View { kind: 1, data, len, ascii: true };
            return Parser::new(b, v).document();
        }
        let d = ffi::PyUnicode_DecodeUTF8(data.cast(), len as ffi::Py_ssize_t, c"surrogatepass".as_ptr());
        if d.is_null() {
            ffi::PyErr_Clear();
            return Err(Stop);
        }
        let r = parse_view(&view(d));
        ffi::Py_DECREF(d);
        r
    }
}

/// The `json.loads` kernel.
pub(super) unsafe extern "C" fn kernel(
    func: *mut ffi::PyObject,
    args: *const *mut ffi::PyObject,
    nargsf: usize,
    kwnames: *mut ffi::PyObject,
) -> *mut ffi::PyObject {
    // SAFETY: vectorcall arguments of `json.loads`; attached thread.
    unsafe {
        let nargs = nargsf & !ffi::PY_VECTORCALL_ARGUMENTS_OFFSET;
        let why = 'fallback: {
            if nargs != 1 || !kwargs_ok(args, nargs, kwnames) {
                break 'fallback Reason::Args;
            }
            let f = func.cast::<ffi::PyFunctionObject>();
            if !DEPS.ok()
                || !super::SHARED_DEPS.ok()
                || func != FUNC.load(Relaxed)
                || (*f).func_code != CODE.load(Relaxed)
                || (*f).func_kwdefaults != KWDEFAULTS.load(Relaxed)
                || ffi::Py_TYPE(DEFAULT_DECODER.load(Relaxed)).cast() != DECODER_TYPE.load(Relaxed)
                || !DECODE.ok()
                || !RAW_DECODE.ok()
            {
                break 'fallback Reason::Deps;
            }
            if !accel::tools_quiet(&TOOLS, func) {
                break 'fallback Reason::Tools;
            }
            match loads(*args) {
                Ok(r) => {
                    if COUNTERS.hit_check_streak() {
                        let all = std::slice::from_raw_parts(
                            args,
                            nargs + if kwnames.is_null() { 0 } else { ffi::PyTuple_GET_SIZE(kwnames) as usize },
                        );
                        return check::verify(&super::LOADS, r, all, || {
                            accel::FUNCTION_VECTORCALL.get()(func, args, nargsf, kwnames)
                        });
                    }
                    return r;
                }
                Err(Stop) => break 'fallback Reason::Deopt,
            }
        };
        COUNTERS.fallback_streak(why);
        accel::FUNCTION_VECTORCALL.get()(func, args, nargsf, kwnames)
    }
}
